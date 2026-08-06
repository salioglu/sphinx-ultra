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
/// - ** matches everything, including directory separators
/// - * matches everything except a directory separator
/// - ? matches any single character except a directory separator
/// - [seq] matches any character in seq
/// - [!seq] matches any character not in seq (never a directory separator)
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
                    // ** matches everything, including '/'. Sphinx has no
                    // directory-boundary special case: a following '/' is an
                    // ordinary literal, so 'foo/**/bar' requires at least one
                    // intermediate path component.
                    regex_pattern.push_str(".*");
                    i += 2;
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
                // Character class: scan for the closing ']' like Sphinx,
                // skipping a leading '!' and then a leading ']' (a ']' in
                // first position is a literal member)
                let mut j = i + 1;
                if j < n && chars[j] == '!' {
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
                    // Valid character class.
                    // Sphinx semantics (sphinx/util/matching.py): backslashes
                    // in the class body are doubled (so '[\d]' is a literal
                    // backslash or 'd', never the digit class), only '[!...]'
                    // negates and never matches '/', and a leading '^' is an
                    // escaped literal character.
                    let body: String = chars[i + 1..j].iter().collect();
                    let mut stuff = body.replace('\\', "\\\\");
                    if let Some(rest) = stuff.strip_prefix('!') {
                        stuff = format!("^/{rest}");
                    } else if stuff.starts_with('^') {
                        stuff.insert(0, '\\');
                    }

                    regex_pattern.push('[');
                    regex_pattern.push_str(&stuff);
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
                // Prune excluded directories like Sphinx's get_matching_files,
                // which filters os.walk dirs by matching the bare relative path
                // against the exclude matchers (a trailing-slash pattern like
                // "_build/" is inert in Sphinx and must stay inert here).
                let relative_path = path.strip_prefix(base_dir)?;
                let normalized_path = normalize_path(relative_path);

                let excluded = exclude_regexes
                    .iter()
                    .any(|regex| regex.is_match(&normalized_path));

                if !excluded {
                    walk_dir(
                        &path,
                        base_dir,
                        include_regexes,
                        exclude_regexes,
                        matched_files,
                    )?;
                }
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
        assert_eq!(translate_pattern("**/index.rst"), "^.*/index\\.rst$");
        assert_eq!(translate_pattern("docs/*.rst"), "^docs/[^/]*\\.rst$");
        assert_eq!(translate_pattern("***"), "^.*[^/]*$");

        // Character classes
        assert_eq!(translate_pattern("[abc].rst"), "^[abc]\\.rst$");
        assert_eq!(translate_pattern("[!abc].rst"), "^[^/abc]\\.rst$");
        assert_eq!(translate_pattern("[^abc].rst"), "^[\\^abc]\\.rst$");
        assert_eq!(translate_pattern("[\\d]x"), "^[\\\\d]x$");
    }

    #[test]
    fn test_double_star_matches_sphinx() {
        // Sphinx translates '**' to plain '.*' with no directory-boundary
        // special case, so 'foo/**/bar' requires at least one intermediate
        // component and '**/x' never matches a top-level 'x'.
        assert!(!pattern_match("foo/bar", "foo/**/bar").unwrap());
        assert!(pattern_match("foo/x/bar", "foo/**/bar").unwrap());
        assert!(pattern_match("foo/x/y/bar", "foo/**/bar").unwrap());
        assert!(!pattern_match("bar", "**/bar").unwrap());
        assert!(pattern_match("x/bar", "**/bar").unwrap());
        assert!(!pattern_match("foo", "foo/**").unwrap());
        assert!(pattern_match("foo/x/y", "foo/**").unwrap());
        assert!(pattern_match("foo/bar.rst", "**").unwrap());
        assert!(!pattern_match("foo/bar", "*").unwrap());
        assert!(pattern_match("foo", "*").unwrap());
        assert!(!pattern_match("a/b.rst", "*.rst").unwrap());
        assert!(pattern_match("a/b.rst", "**.rst").unwrap());
        assert!(pattern_match("ab", "a**b").unwrap());
        assert!(pattern_match("axx/yyb", "a**b").unwrap());
    }

    #[test]
    fn test_character_class_matches_sphinx() {
        assert!(pattern_match("bx", "[!a]x").unwrap());
        assert!(!pattern_match("/x", "[!a]x").unwrap());
        // '[^...]' does not negate: the caret is an escaped literal member
        assert!(pattern_match("^x", "[^a]x").unwrap());
        assert!(pattern_match("ax", "[^a]x").unwrap());
        assert!(!pattern_match("bx", "[^a]x").unwrap());
        // Sphinx doubles in-class backslashes, so '[\d]' is a class of a
        // literal backslash and 'd', never the regex digit class
        assert!(pattern_match("dx", "[\\d]x").unwrap());
        assert!(!pattern_match("5x", "[\\d]x").unwrap());
        assert!(pattern_match("\\x", "[\\d]x").unwrap());
        // A ']' first in the class body is a literal member, exactly as
        // Python's re parses Sphinx's output
        assert!(pattern_match("]", "[]a]").unwrap());
        assert!(pattern_match("a", "[]a]").unwrap());
        // '[!]a]' becomes '[^/]a]': the ']' closes the class early,
        // leaving a literal 'a]' tail
        assert!(pattern_match("]a]", "[!]a]").unwrap());
        assert!(pattern_match("xa]", "[!]a]").unwrap());
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
        // Sphinx semantics: negated classes never match '/'
        assert!(!pattern_match("/.rst", "[!abc].rst").unwrap());
    }

    #[test]
    fn test_directory_pruning_matches_sphinx() {
        // Sphinx's get_matching_files prunes directories whose bare relative
        // path matches an exclude matcher; "_build/**" excludes the files
        // beneath instead; a trailing-slash pattern like "_build/" matches
        // neither directories nor files (inert).
        let make_tree = || {
            let temp_dir = TempDir::new().unwrap();
            let base = temp_dir.path().to_path_buf();
            fs::create_dir_all(base.join("_build/deep")).unwrap();
            fs::write(base.join("index.rst"), "Index").unwrap();
            fs::write(base.join("_build/stale.rst"), "Stale").unwrap();
            fs::write(base.join("_build/deep/stale.rst"), "Stale").unwrap();
            (temp_dir, base)
        };

        let include = vec!["**".to_string()];

        // Bare "_build" prunes the whole tree
        let (_t1, base) = make_tree();
        let files = get_matching_files(&base, &include, &["_build".to_string()]).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|f| normalize_path(f.strip_prefix(base.canonicalize().unwrap()).unwrap()))
            .collect();
        assert_eq!(names, vec!["index.rst"]);

        // "_build/**" produces the same visible output (files excluded one by one)
        let (_t2, base) = make_tree();
        let files = get_matching_files(&base, &include, &["_build/**".to_string()]).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|f| normalize_path(f.strip_prefix(base.canonicalize().unwrap()).unwrap()))
            .collect();
        assert_eq!(names, vec!["index.rst"]);

        // Trailing-slash "_build/" is inert, exactly like Sphinx
        let (_t3, base) = make_tree();
        let files = get_matching_files(&base, &include, &["_build/".to_string()]).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|f| normalize_path(f.strip_prefix(base.canonicalize().unwrap()).unwrap()))
            .collect();
        assert_eq!(
            names,
            vec!["_build/deep/stale.rst", "_build/stale.rst", "index.rst"]
        );
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

        // Test include nested RST files: '**/*.rst' requires a directory
        // component in Sphinx, so top-level index.rst is not matched
        let files = get_matching_files(base_path, &["**/*.rst".to_string()], &[]).unwrap();
        assert_eq!(files.len(), 1);
        assert!(!files.iter().any(|p| p.file_name().unwrap() == "index.rst"));
        assert!(files.iter().any(|p| p.file_name().unwrap() == "api.rst"));

        // Test exclude _build directory
        let files =
            get_matching_files(base_path, &["**".to_string()], &["_build/**".to_string()]).unwrap();
        assert!(!files.iter().any(|p| p.to_string_lossy().contains("_build")));

        // Test include RST files but exclude docs directory: with Sphinx
        // '**' semantics the include only matches docs/api.rst, which the
        // exclusion then removes
        let files = get_matching_files(
            base_path,
            &["**/*.rst".to_string()],
            &["docs/**".to_string()],
        )
        .unwrap();
        assert!(files.is_empty());
    }
}
