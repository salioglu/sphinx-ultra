//! Pattern matching utilities for file filtering.
//!
//! This module provides glob-style pattern matching compatible with Sphinx's
//! include_patterns and exclude_patterns functionality. It implements the same
//! pattern translation and matching logic as Sphinx's util/matching.py.

use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

lazy_static::lazy_static! {
    /// Cache for compiled regex patterns
    static ref PATTERN_CACHE: Mutex<HashMap<String, Regex>> = Mutex::new(HashMap::new());
}

/// Translates shell-style glob pattern to regex pattern.
///
/// This implements the same logic as Sphinx's _translate_pattern function:
/// - ** matches any files and zero or more directories and subdirectories  
/// - * matches everything except a directory separator
/// - ? matches any single character except a directory separator
/// - [seq] matches any character in seq
/// - [!seq] matches any character not in seq
///
/// Based on Python's fnmatch.translate but with modifications for path handling.
pub fn translate_pattern(pattern: &str) -> String {
    let mut regex_pattern = String::new();
    let mut i = 0;
    let chars: Vec<char> = pattern.chars().collect();
    let n = chars.len();

    while i < n {
        let c = chars[i];
        match c {
            '*' => {
                if i + 1 < n && chars[i + 1] == '*' {
                    // Handle ** - matches any files and directories
                    if i + 2 < n && chars[i + 2] == '/' {
                        // **/
                        regex_pattern.push_str("(?:[^/]+/)*");
                        i += 3;
                    } else if i + 2 == n {
                        // ** at end
                        regex_pattern.push_str(".*");
                        i += 2;
                    } else {
                        // **something
                        regex_pattern.push_str(".*");
                        i += 2;
                    }
                } else {
                    // Single * - matches everything except directory separator
                    regex_pattern.push_str("[^/]*");
                    i += 1;
                }
            }
            '?' => {
                // ? matches any single character except directory separator
                regex_pattern.push_str("[^/]");
                i += 1;
            }
            '[' => {
                // Character class
                let mut j = i + 1;
                if j < n && (chars[j] == '!' || chars[j] == '^') {
                    j += 1;
                }
                if j < n && chars[j] == ']' {
                    j += 1;
                }
                while j < n && chars[j] != ']' {
                    j += 1;
                }
                if j >= n {
                    // No closing ], treat [ as literal
                    regex_pattern.push_str("\\[");
                    i += 1;
                } else {
                    // Valid character class
                    let mut class_content = String::new();
                    let mut k = i + 1;

                    if k < n && (chars[k] == '!' || chars[k] == '^') {
                        class_content.push('^');
                        k += 1;
                    }

                    while k < j {
                        let ch = chars[k];
                        if ch == '\\' && k + 1 < j {
                            class_content.push('\\');
                            class_content.push(chars[k + 1]);
                            k += 2;
                        } else {
                            class_content.push(ch);
                            k += 1;
                        }
                    }

                    regex_pattern.push('[');
                    regex_pattern.push_str(&class_content);
                    regex_pattern.push(']');
                    i = j + 1;
                }
            }
            _ => {
                // Escape regex special characters
                match c {
                    '\\' | '.' | '^' | '$' | '+' | '{' | '}' | '|' | '(' | ')' => {
                        regex_pattern.push('\\');
                        regex_pattern.push(c);
                    }
                    _ => {
                        regex_pattern.push(c);
                    }
                }
                i += 1;
            }
        }
    }

    // Anchor the pattern to match the entire string
    format!("^{}$", regex_pattern)
}

/// Compiles a pattern into a regex, using cache for performance.
pub fn compile_pattern(pattern: &str) -> Result<Regex, regex::Error> {
    let mut cache = PATTERN_CACHE.lock().unwrap();

    if let Some(regex) = cache.get(pattern) {
        return Ok(regex.clone());
    }

    let regex_pattern = translate_pattern(pattern);
    let regex = Regex::new(&regex_pattern)?;
    cache.insert(pattern.to_string(), regex.clone());

    Ok(regex)
}

/// Tests if a name matches a glob pattern.
pub fn pattern_match(name: &str, pattern: &str) -> Result<bool, regex::Error> {
    let regex = compile_pattern(pattern)?;
    Ok(regex.is_match(name))
}

/// Filters a list of names by a glob pattern.
pub fn pattern_filter(names: &[String], pattern: &str) -> Result<Vec<String>, regex::Error> {
    let regex = compile_pattern(pattern)?;
    Ok(names
        .iter()
        .filter(|name| regex.is_match(name))
        .cloned()
        .collect())
}

/// Normalizes a path to use forward slashes for pattern matching.
/// This ensures consistent behavior across platforms.
pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Gets matching files from a directory using include and exclude patterns.
///
/// This function implements the same logic as Sphinx's get_matching_files:
/// - Only files matching some pattern in include_patterns are included
/// - Exclusions from exclude_patterns take priority over inclusions
/// - The default include pattern is "**" (all files)
/// - The default exclude pattern is empty (exclude nothing)
pub fn get_matching_files<P: AsRef<Path>>(
    dirname: P,
    include_patterns: &[String],
    exclude_patterns: &[String],
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let dirname = dirname.as_ref().canonicalize()?;
    let include_patterns = if include_patterns.is_empty() {
        vec!["**".to_string()]
    } else {
        include_patterns.to_vec()
    };

    // Compile all patterns
    let mut include_regexes = Vec::new();
    for pattern in &include_patterns {
        include_regexes.push(compile_pattern(pattern)?);
    }

    let mut exclude_regexes = Vec::new();
    for pattern in exclude_patterns {
        exclude_regexes.push(compile_pattern(pattern)?);
    }

    let mut matched_files = Vec::new();

    // Walk the directory recursively
    fn walk_dir(
        dir: &Path,
        base_dir: &Path,
        include_regexes: &[Regex],
        exclude_regexes: &[Regex],
        matched_files: &mut Vec<PathBuf>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if !dir.is_dir() {
            return Ok(());
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Recursively walk subdirectories
                walk_dir(
                    &path,
                    base_dir,
                    include_regexes,
                    exclude_regexes,
                    matched_files,
                )?;
            } else if path.is_file() {
                // Get relative path from base directory
                let relative_path = path.strip_prefix(base_dir)?;
                let normalized_path = normalize_path(relative_path);

                // Check if file matches any include pattern
                let included = include_regexes
                    .iter()
                    .any(|regex| regex.is_match(&normalized_path));

                if included {
                    // Check if file matches any exclude pattern
                    let excluded = exclude_regexes
                        .iter()
                        .any(|regex| regex.is_match(&normalized_path));

                    if !excluded {
                        matched_files.push(path);
                    }
                }
            }
        }

        Ok(())
    }

    walk_dir(
        &dirname,
        &dirname,
        &include_regexes,
        &exclude_regexes,
        &mut matched_files,
    )?;

    // Sort for consistent results
    matched_files.sort();

    Ok(matched_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_translate_pattern() {
        // Basic patterns
        assert_eq!(translate_pattern("*.rst"), "^[^/]*\\.rst$");
        assert_eq!(translate_pattern("**"), "^.*$");
        assert_eq!(
            translate_pattern("**/index.rst"),
            "^(?:[^/]+/)*index\\.rst$"
        );
        assert_eq!(translate_pattern("docs/*.rst"), "^docs/[^/]*\\.rst$");

        // Character classes
        assert_eq!(translate_pattern("[abc].rst"), "^[abc]\\.rst$");
        assert_eq!(translate_pattern("[!abc].rst"), "^[^abc]\\.rst$");
    }

    #[test]
    fn test_pattern_match() {
        // Test basic patterns
        assert!(pattern_match("index.rst", "*.rst").unwrap());
        assert!(pattern_match("docs/index.rst", "**/*.rst").unwrap());
        assert!(pattern_match("docs/api/module.rst", "**/api/*.rst").unwrap());

        // Test exclusions
        assert!(!pattern_match("_build/index.html", "*.rst").unwrap());
        assert!(pattern_match("_build/index.html", "**").unwrap());

        // Test character classes
        assert!(pattern_match("a.rst", "[abc].rst").unwrap());
        assert!(!pattern_match("d.rst", "[abc].rst").unwrap());
        assert!(!pattern_match("a.rst", "[!abc].rst").unwrap());
        assert!(pattern_match("d.rst", "[!abc].rst").unwrap());
    }

    #[test]
    fn test_get_matching_files() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        // Create test files
        fs::create_dir_all(base_path.join("docs")).unwrap();
        fs::create_dir_all(base_path.join("_build")).unwrap();
        fs::write(base_path.join("index.rst"), "content").unwrap();
        fs::write(base_path.join("docs/api.rst"), "content").unwrap();
        fs::write(base_path.join("_build/index.html"), "content").unwrap();
        fs::write(base_path.join("README.md"), "content").unwrap();

        // Test include all RST files
        let files = get_matching_files(base_path, &["**/*.rst".to_string()], &[]).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|p| p.file_name().unwrap() == "index.rst"));
        assert!(files.iter().any(|p| p.file_name().unwrap() == "api.rst"));

        // Test exclude _build directory
        let files =
            get_matching_files(base_path, &["**".to_string()], &["_build/**".to_string()]).unwrap();
        assert!(!files.iter().any(|p| p.to_string_lossy().contains("_build")));

        // Test include RST files but exclude docs directory
        let files = get_matching_files(
            base_path,
            &["**/*.rst".to_string()],
            &["docs/**".to_string()],
        )
        .unwrap();
        assert_eq!(files.len(), 1);
        assert!(files.iter().any(|p| p.file_name().unwrap() == "index.rst"));
        assert!(!files.iter().any(|p| p.file_name().unwrap() == "api.rst"));
    }
}
