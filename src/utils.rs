use anyhow::Result;
use chrono::{DateTime, Utc};
use std::path::Path;

#[derive(Debug)]
pub struct ProjectStats {
    pub source_files: usize,
    pub total_lines: usize,
    pub avg_file_size_kb: f64,
    pub largest_file_kb: f64,
    pub max_depth: usize,
    pub cross_references: usize,
}

pub async fn analyze_project(source_dir: &Path) -> Result<ProjectStats> {
    let mut state = AnalysisState {
        source_files: 0,
        total_lines: 0,
        total_size_bytes: 0,
        largest_file_kb: 0.0,
        max_depth: 0,
        cross_references: 0,
    };

    // Use synchronous approach to avoid async recursion issues
    analyze_directory_sync(source_dir, source_dir, 0, &mut state)?;

    let avg_file_size_kb = if state.source_files > 0 {
        (state.total_size_bytes as f64) / (state.source_files as f64) / 1024.0
    } else {
        0.0
    };

    Ok(ProjectStats {
        source_files: state.source_files,
        total_lines: state.total_lines,
        avg_file_size_kb,
        largest_file_kb: state.largest_file_kb,
        max_depth: state.max_depth,
        cross_references: state.cross_references,
    })
}

/// Analysis state for directory traversal
struct AnalysisState {
    source_files: usize,
    total_lines: usize,
    total_size_bytes: u64,
    largest_file_kb: f64,
    max_depth: usize,
    cross_references: usize,
}

fn analyze_directory_sync(
    dir: &Path,
    _root_dir: &Path,
    current_depth: usize,
    state: &mut AnalysisState,
) -> Result<()> {
    state.max_depth = state.max_depth.max(current_depth);

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // Skip hidden directories
            if let Some(name) = path.file_name() {
                if name.to_string_lossy().starts_with('.') {
                    continue;
                }
            }

            analyze_directory_sync(&path, _root_dir, current_depth + 1, state)?;
        } else if is_source_file(&path) {
            state.source_files += 1;

            let metadata = std::fs::metadata(&path)?;
            let file_size_bytes = metadata.len();
            let file_size_kb = file_size_bytes as f64 / 1024.0;

            state.total_size_bytes += file_size_bytes;
            state.largest_file_kb = state.largest_file_kb.max(file_size_kb);

            // Count lines and cross-references
            if let Ok(content) = std::fs::read_to_string(&path) {
                state.total_lines += content.lines().count();
                state.cross_references += count_cross_references(&content);
            }
        }
    }

    Ok(())
}

pub fn is_source_file(path: &Path) -> bool {
    if let Some(ext) = path.extension() {
        matches!(ext.to_string_lossy().as_ref(), "rst" | "md" | "txt")
    } else {
        false
    }
}

pub fn count_cross_references(content: &str) -> usize {
    let patterns = [
        r":doc:`",
        r":ref:`",
        r":func:`",
        r":class:`",
        r":meth:`",
        r":attr:`",
        r":mod:`",
        r":py:",
        r".. _",
        r"`~",
    ];

    let mut count = 0;
    for pattern in &patterns {
        count += content.matches(pattern).count();
    }
    count
}

pub fn get_file_mtime(path: &Path) -> Result<DateTime<Utc>> {
    let metadata = std::fs::metadata(path)?;
    let mtime = metadata.modified()?;
    Ok(DateTime::from(mtime))
}

pub async fn calculate_directory_size(dir: &Path) -> Result<u64> {
    // Use synchronous approach
    calculate_directory_size_sync(dir)
}

fn calculate_directory_size_sync(dir: &Path) -> Result<u64> {
    let mut total_size = 0;

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            total_size += calculate_directory_size_sync(&path)?;
        } else {
            let metadata = std::fs::metadata(&path)?;
            total_size += metadata.len();
        }
    }

    Ok(total_size)
}

pub async fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    // Use synchronous approach
    copy_dir_recursive_sync(src, dst)
}

fn copy_dir_recursive_sync(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive_sync(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

pub fn format_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    if secs > 0 {
        format!("{}.{:03}s", secs, millis)
    } else {
        format!("{}ms", millis)
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];

    if bytes == 0 {
        return "0 B".to_string();
    }

    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{:.1} {}", size, UNITS[unit_index])
}

/// Format a date according to the specified format string and language
pub fn format_date(fmt: &str, _language: &Option<String>) -> String {
    let now = chrono::Utc::now();

    match fmt {
        "%b %d, %Y" => now.format("%b %d, %Y").to_string(),
        "%B %d, %Y" => now.format("%B %d, %Y").to_string(),
        "%Y-%m-%d" => now.format("%Y-%m-%d").to_string(),
        "%Y-%m-%d %H:%M:%S" => now.format("%Y-%m-%d %H:%M:%S").to_string(),
        _ => {
            // For custom formats, try to parse and format
            match chrono::DateTime::parse_from_str(&now.to_rfc3339(), "%+") {
                Ok(dt) => dt.format(fmt).to_string(),
                Err(_) => now.format("%Y-%m-%d").to_string(),
            }
        }
    }
}

/// Ensure a directory exists, creating it if necessary
pub async fn ensure_dir(path: &Path) -> Result<()> {
    use tokio::fs;

    if !path.exists() {
        fs::create_dir_all(path).await?;
    }
    Ok(())
}

/// Calculate relative URI from one path to another
pub fn relative_uri(from: &str, to: &str, suffix: &str) -> String {
    use std::path::Path;

    let from_path = Path::new(from);
    let to_path = Path::new(to);

    // Get the relative path
    if let Some(rel_path) =
        pathdiff::diff_paths(to_path, from_path.parent().unwrap_or(Path::new("")))
    {
        let mut result = rel_path.to_string_lossy().to_string();
        if !suffix.is_empty() && !result.ends_with(suffix) {
            result.push_str(suffix);
        }
        result.replace('\\', "/") // Ensure forward slashes
    } else {
        format!("{}{}", to, suffix)
    }
}

/// Copy all files and directories from source to destination
pub async fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    use tokio::fs;

    ensure_dir(dst).await?;

    let mut entries = fs::read_dir(src).await?;

    while let Some(entry) = entries.next_entry().await? {
        let entry_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dst.join(file_name);

        if entry_path.is_dir() {
            Box::pin(copy_dir_all(&entry_path, &dest_path)).await?;
        } else {
            if let Some(parent) = dest_path.parent() {
                ensure_dir(parent).await?;
            }
            fs::copy(&entry_path, &dest_path).await?;
        }
    }

    Ok(())
}
