use anyhow::Result;
use chrono::{DateTime, Utc};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// [`std::fs::canonicalize`] with the Windows verbatim prefix taken back
/// off ([`simplify_verbatim`]) — the spelling every path comparison,
/// display and `%r` in this crate speaks.
///
/// EVERY canonicalization in the tree (its tests included) goes through
/// here: the prefix has to be present on all sides of a comparison or on
/// none, and "none" is what Python produces, so "none" it is.
pub fn canonicalize_simplified(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(simplify_verbatim)
}

/// Drop the `\\?\` verbatim prefix that Windows' `GetFinalPathNameByHandle`
/// — and so [`std::fs::canonicalize`] — puts in front of every canonical
/// path.
///
/// Python's `pathlib.Path.resolve()` and `os.path.realpath()` return the
/// plain `C:\dir\file` spelling, so the verbatim form is a byte-divergence
/// in every message that prints a resolved path (`_StrPath(...)` reprs,
/// `Include file '...'`, `:diff:` headers). It is also a FUNCTIONAL
/// hazard: inside a verbatim path Windows does no normalization at all —
/// `/` is an ordinary filename character there and `..` is not resolved —
/// so a lexically joined `\\?\C:\src` + `sub/inner.rst` names nothing.
///
/// `\\?\UNC\server\share` maps back to `\\server\share`; a path long
/// enough that the prefix is what makes it openable at all keeps it (the
/// 260-character `MAX_PATH` limit, which the plain spelling is only exempt
/// from with a per-application opt-in this crate does not make). A
/// non-Windows path matches neither shape, so this is the identity
/// function there.
pub fn simplify_verbatim(path: PathBuf) -> PathBuf {
    let simplified = match path.to_str() {
        Some(text) => match simplify_verbatim_str(text) {
            Cow::Borrowed(unchanged) if unchanged.len() == text.len() => None,
            simplified => Some(simplified.into_owned()),
        },
        None => None,
    };
    match simplified {
        Some(text) => PathBuf::from(text),
        None => path,
    }
}

/// The string half of [`simplify_verbatim`], so the rule can be tested
/// with Windows-shaped literals on every platform.
fn simplify_verbatim_str(text: &str) -> Cow<'_, str> {
    /// `MAX_PATH`: at this length the plain spelling stops being openable,
    /// so the verbatim prefix stays on.
    const MAX_PATH: usize = 260;

    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        let plain = format!(r"\\{share}");
        if plain.len() < MAX_PATH {
            return Cow::Owned(plain);
        }
        return Cow::Borrowed(text);
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        // Only `X:\...` survives the round trip; the other verbatim shapes
        // (`\\?\Volume{...}`, a device path) have no plain spelling.
        let mut head = rest.chars();
        let drive = matches!(
            (head.next(), head.next(), head.next()),
            (Some(letter), Some(':'), Some('\\')) if letter.is_ascii_alphabetic()
        );
        if drive && rest.len() < MAX_PATH {
            return Cow::Borrowed(rest);
        }
    }
    Cow::Borrowed(text)
}

/// `BuildEnvironment.relfn2path` (`environment/__init__.py:454-478`): a
/// filename written in a document resolves relative to that document's
/// directory, unless it is written absolute (`/pic.png`), in which case it
/// is relative to the source directory. The result is normalized (`.` and
/// `..` collapsed, Sphinx's `os.path.normpath`) and joined onto srcdir.
///
/// Shared home (wave 4.5): the image dependency collector
/// ([`crate::env::dependencies`]) and the `include` directive's sphinx-mode
/// path rewrite (`sphinx/directives/other.py:413-416`) both resolve through
/// this port.
pub fn relfn2path(uri: &str, docname: &str, srcdir: &Path) -> PathBuf {
    let mut path = srcdir.to_path_buf();
    for segment in relfn2path_rel(uri, docname).split('/') {
        if !segment.is_empty() {
            path.push(segment);
        }
    }
    path
}

/// The path to actually OPEN for `uri` written in `docname`.
///
/// Sphinx's `relfn2path` calls `.resolve()` on the joined path
/// (`environment/__init__.py:466`/`:475`), so symlinks are followed BEFORE
/// any `..` is interpreted. [`relfn2path`] instead collapses `..`
/// lexically, which under a symlinked directory — `docs/examples ->
/// ../../examples` and an `include` of `examples/../shared.txt` — names a
/// DIFFERENT FILE: sphinx walks up from the link's target, the lexical
/// rule walks up from the link's own parent.
///
/// The two are separate functions on purpose. §Scope-8 fixes the
/// srcdir-relative spelling every path-bearing surface of included
/// content shows, so [`relfn2path`] keeps feeding those; only the read
/// goes through here.
pub fn relfn2path_io(uri: &str, docname: &str, srcdir: &Path) -> PathBuf {
    let mut path = srcdir.to_path_buf();
    for segment in relfn2path_join(uri, docname).split('/') {
        if !segment.is_empty() {
            path.push(segment);
        }
    }
    resolve_path(&path)
}

/// The join `relfn2path` does before resolving — `srcdir.joinpath(doc_dir,
/// file_name)` — with the `.`/`..` segments still IN. Sphinx collapses
/// them inside `.resolve()`, i.e. AFTER symlinks, so they must not be
/// collapsed here.
fn relfn2path_join(uri: &str, docname: &str) -> String {
    match uri.strip_prefix('/') {
        Some(rooted) => rooted.to_string(),
        None => match docname.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/{uri}"),
            None => uri.to_string(),
        },
    }
}

/// `pathlib.Path.resolve()` with `strict=False`, which is what
/// `relfn2path` calls: symlinks are followed component by component and
/// `..` is applied to what is already RESOLVED, so a path leaving a
/// symlinked directory lands beside the link's target, not beside the
/// link. Components past the last existing one cannot be followed and
/// collapse lexically, exactly as `os.path.realpath(strict=False)` does —
/// which is how a not-yet-existing include target still normalizes.
pub(crate) fn resolve_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
                if let Ok(real) = canonicalize_simplified(&resolved) {
                    resolved = real;
                }
            }
        }
    }
    resolved
}

/// The srcdir-relative half of [`relfn2path`]: the normalized posix path
/// (relative to the source directory) that `uri` written in `docname`
/// refers to. Sphinx's `rel_fn` return value.
pub fn relfn2path_rel(uri: &str, docname: &str) -> String {
    let relative = match uri.strip_prefix('/') {
        Some(rooted) => rooted.to_string(),
        None => match docname.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/{uri}"),
            None => uri.to_string(),
        },
    };
    normalize_dot_segments(&relative)
}

/// `os.path.normpath` over a posix-separated relative path: `.` and inner
/// `..` collapse; a leading `..` stays and walks out of the tree (a path
/// that simply will not exist).
pub fn normalize_dot_segments(relative: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in relative.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                // `normpath` only drops a `..` that has something to undo.
                if matches!(segments.last(), Some(&last) if last != "..") {
                    segments.pop();
                } else {
                    segments.push("..");
                }
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

/// `Path.relative_to(root, walk_up=True)` over two already-resolved
/// absolute paths — the second half of sphinx's `relfn2path`
/// (`_relative_path(abs_fn, self.srcdir)`, `util/osutil.py:173-189`):
/// the srcdir-relative spelling of a file, walking UP with `..` when the
/// file lies outside the source directory (`srcdir / "../ext/part.rst"`
/// is exactly what `note_dependency` then stores). Posix-separated.
///
/// Both inputs must be resolved ([`resolve_path`]), like sphinx's — a
/// symlink followed on one side only would make the walk-up lie. Paths on
/// different roots (Windows drives) have no relative spelling; sphinx
/// returns the path itself, and so does this.
pub(crate) fn relative_path_walk_up(path: &Path, root: &Path) -> String {
    use std::path::Component;
    let path_parts: Vec<Component<'_>> = path.components().collect();
    let root_parts: Vec<Component<'_>> = root.components().collect();
    let anchors_differ = match (path_parts.first(), root_parts.first()) {
        (Some(Component::Prefix(a)), Some(Component::Prefix(b))) => a != b,
        (Some(Component::Prefix(_)), _) | (_, Some(Component::Prefix(_))) => true,
        _ => false,
    };
    if anchors_differ {
        return path.to_string_lossy().replace('\\', "/");
    }
    let common = path_parts
        .iter()
        .zip(root_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut segments: Vec<String> = vec!["..".to_string(); root_parts.len() - common];
    segments.extend(
        path_parts[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    segments.join("/")
}

/// Python `str.isspace()`, which is also what `re`'s `\s` matches on a
/// `str` pattern: the Unicode White_Space set ([`char::is_whitespace`])
/// PLUS the four C0 separators `\x1c`-`\x1f` (bidirectional class B/S,
/// which Unicode does not call whitespace). `\x1c`-`\x1e` are also
/// [`py_splitlines`] boundaries; `\x1f` is the one that survives into a
/// line and shows the difference.
pub(crate) fn py_isspace(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\x1c'..='\x1f')
}

/// The first code point of every run of Unicode decimal digits
/// (`Numeric_Type=Decimal`, general category `Nd`) — each run is exactly
/// `0`-`9`, a Unicode stability guarantee — as of Unicode 15.0.0, the
/// version CPython 3.12's `unicodedata` carries (generated from
/// `unicodedata.decimal`: 68 runs, 680 digits — the same 680 characters
/// `re`'s `\d` matches on a `str` pattern).
pub(crate) const PY_DECIMAL_ZEROS: [u32; 68] = [
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66,
    0xDE6, 0xE50, 0xED0, 0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90,
    0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    0x104A0, 0x10D30, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0, 0x11650,
    0x116C0, 0x11730, 0x118E0, 0x11950, 0x11C50, 0x11D50, 0x11DA0, 0x11F50, 0x16A60, 0x16AC0,
    0x16B50, 0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E4F0, 0x1E950,
    0x1FBF0,
];

/// Python's `unicodedata.decimal(c, None)`: the digit a Unicode decimal
/// digit stands for ([`PY_DECIMAL_ZEROS`]).
pub(crate) fn py_decimal(c: char) -> Option<u8> {
    let code = u32::from(c);
    let run = PY_DECIMAL_ZEROS
        .partition_point(|&zero| zero <= code)
        .checked_sub(1)?;
    u8::try_from(code - PY_DECIMAL_ZEROS[run])
        .ok()
        .filter(|digit| *digit < 10)
}

/// `repr()` of a Python `str` (CPython `unicode_repr`): single quotes
/// unless the text has a `'` and no `"`; `\\`, `\n`, `\r`, `\t` and the
/// chosen quote backslash-escaped; and every character
/// `str.isprintable()` rejects rendered as `\xNN` / `\uNNNN` / `\UNNNNNNNN`
/// (lowercase hex). That set is the categories Cc, Cf, Cs, Co, Cn, Zl, Zp
/// and Zs minus the ASCII space: `is_control()` is exactly Cc and
/// `is_whitespace()` is exactly Zs|Zl|Zp plus Cc members, so the predicate
/// below is those five categories precisely, and Cs cannot exist in a Rust
/// `char`. **Cf, Co and Cn are the ledgered gap** — matching them needs a
/// Unicode general-category table this tree does not have — recorded in
/// docs/IMPLEMENTATION_STATUS.md and nowhere else; this is the ONE
/// implementation (panel fix round F), behind `src/rst/block.rs`'s
/// `py_repr` for directive messages and index-entry tuples and behind
/// every warning-stream `%r` (toctree, resolver, py domain, intersphinx,
/// builder).
pub(crate) fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if c != ' ' && (c.is_control() || c.is_whitespace()) => {
                let n = c as u32;
                if n <= 0xff {
                    out.push_str(&format!("\\x{n:02x}"));
                } else if n <= 0xffff {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `str.split()` with no separator: split on runs of [`py_isspace`],
/// dropping the empty leading/trailing/interior fields. Rust's
/// `str::split_whitespace` is the same shape over a NARROWER set (it misses
/// the C0 separators `\x1c`-`\x1f`), so every port of a docutils/Sphinx
/// `.split()` must come through here.
pub(crate) fn py_split(s: &str) -> impl Iterator<Item = &str> {
    s.split(py_isspace).filter(|w| !w.is_empty())
}

/// `Project.path2doc` (`sphinx/project.py:114-128`) against Sphinx's
/// *default* `source_suffix` — `{'.rst': 'restructuredtext'}`
/// (`config.py:243`): the docname a source file under `srcdir` maps to, or
/// `None` for anything else.
///
/// Deliberately narrower than this crate's discovery (which also admits
/// `.md`/`.txt`): the consumers — `env.included` bookkeeping and the orphan
/// check behind it — must match what Sphinx records, and Sphinx's default
/// project never maps a `.txt` include target to a document.
///
/// One knowing simplification: for a `.rst` OUTSIDE `srcdir` sphinx's
/// `relative_to` fails and the absolute path itself becomes the "docname"
/// (`'/base/ext/part'`, probed) — a name no document can have, so the
/// orphan check ignores it. `None` here has the same effect, and keeps
/// `env.included` free of environment-specific absolute paths.
pub fn path2doc(path: &Path, srcdir: &Path) -> Option<String> {
    let rel = path.strip_prefix(srcdir).ok()?;
    let rel = rel.to_str()?.replace('\\', "/");
    Some(rel.strip_suffix(".rst")?.to_string())
}

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

#[allow(dead_code)]
pub fn format_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    if secs > 0 {
        format!("{}.{:03}s", secs, millis)
    } else {
        format!("{}ms", millis)
    }
}

#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
pub async fn ensure_dir(path: &Path) -> Result<()> {
    use tokio::fs;

    if !path.exists() {
        fs::create_dir_all(path).await?;
    }
    Ok(())
}

/// Calculate relative URI from one path to another
#[allow(dead_code)]
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
#[allow(dead_code)]
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

/// Python `str.splitlines()`: the full boundary set (`\n`, `\r`, `\r\n`,
/// `\v`, `\f`, `\x1c`-`\x1e`, `\u{85}`, `\u{2028}`, `\u{2029}`), no
/// trailing empty line for a terminal boundary.
///
/// Shared home: the block parser's line handling and
/// [`crate::doctree::pformat`]'s `Text.pformat` port both need it.
pub(crate) fn py_splitlines(text: &str) -> Vec<&str> {
    let is_boundary = |c: char| {
        matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        )
    };
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if is_boundary(c) {
            out.push(&text[start..i]);
            if c == '\r' {
                if let Some(&(_, '\n')) = chars.peek() {
                    chars.next();
                }
            }
            start = chars.peek().map(|&(j, _)| j).unwrap_or(text.len());
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn relfn2path_rel_resolves_docname_relative_and_rooted_forms() {
        assert_eq!(
            relfn2path_rel("part.rst", "chapters/intro"),
            "chapters/part.rst"
        );
        assert_eq!(
            relfn2path_rel("/sub/abs.rst", "chapters/intro"),
            "sub/abs.rst"
        );
        assert_eq!(
            relfn2path_rel("../img/./pic.png", "chapters/intro"),
            "img/pic.png"
        );
        assert_eq!(relfn2path_rel("x.rst", "index"), "x.rst");
        // A leading `..` walks out of the tree and stays.
        assert_eq!(relfn2path_rel("../outside.rst", "index"), "../outside.rst");
    }

    #[test]
    fn relfn2path_joins_the_rel_half_onto_srcdir() {
        assert_eq!(
            relfn2path("part.rst", "chapters/intro", Path::new("/src")),
            PathBuf::from("/src/chapters/part.rst")
        );
    }

    /// Sphinx's default `source_suffix` is `.rst` alone; this crate's wider
    /// discovery (`.md`/`.txt`) deliberately does not leak into the
    /// `env.included` bookkeeping this helper feeds.
    #[test]
    fn path2doc_maps_rst_under_srcdir_and_nothing_else() {
        let srcdir = Path::new("/src");
        assert_eq!(
            path2doc(Path::new("/src/part.rst"), srcdir),
            Some("part".into())
        );
        assert_eq!(
            path2doc(Path::new("/src/sub/abs_part.rst"), srcdir),
            Some("sub/abs_part".into())
        );
        assert_eq!(path2doc(Path::new("/src/data.txt"), srcdir), None);
        assert_eq!(path2doc(Path::new("/src/notes.md"), srcdir), None);
        assert_eq!(path2doc(Path::new("/elsewhere/part.rst"), srcdir), None);
    }

    /// `_relative_path(abs_fn, srcdir)`: inside the tree it is the plain
    /// relative spelling; outside it walks up with `..`, which is what
    /// makes `srcdir / rel` name the file sphinx actually read.
    ///
    // oracle (sphinx 9.1.0, probe_symlink.py s1/s5): an include resolving
    // to BASE/ext/part.rst from srcdir BASE/src records
    // env.dependencies == {'index': {BASE/'src/../ext/part.rst'}}.
    #[test]
    fn relative_path_walk_up_matches_sphinxs_relative_to() {
        let root = Path::new("/base/src");
        assert_eq!(
            relative_path_walk_up(Path::new("/base/src/a/c.rst"), root),
            "a/c.rst"
        );
        assert_eq!(
            relative_path_walk_up(Path::new("/base/ext/part.rst"), root),
            "../ext/part.rst"
        );
        assert_eq!(
            relative_path_walk_up(Path::new("/other/x.txt"), root),
            "../../other/x.txt"
        );
        assert_eq!(relative_path_walk_up(Path::new("/base/src"), root), "");
    }

    /// Each run of the table is ten digits from its zero (spot checks at
    /// both ends of the table and around a run's edge).
    #[test]
    fn py_decimal_follows_unicodedata() {
        assert_eq!(py_decimal('0'), Some(0));
        assert_eq!(py_decimal('9'), Some(9));
        assert_eq!(py_decimal('\u{669}'), Some(9));
        assert_eq!(py_decimal('\u{66a}'), None);
        assert_eq!(py_decimal('\u{1fbf9}'), Some(9));
        assert_eq!(py_decimal('\u{1fbfa}'), None);
        assert_eq!(py_decimal('\u{b2}'), None, "superscript two is No, not Nd");
        assert_eq!(py_decimal('a'), None);
        assert_eq!(py_decimal('\0'), None);
    }

    /// Python's `isspace` set is Unicode White_Space plus `\x1c`-`\x1f`.
    #[test]
    fn py_isspace_is_unicode_whitespace_plus_the_c0_separators() {
        for c in [
            ' ', '\t', '\n', '\u{a0}', '\u{3000}', '\x1c', '\x1d', '\x1e', '\x1f',
        ] {
            assert!(py_isspace(c), "{c:?}");
        }
        for c in ['a', '\x00', '\x1b', '\u{200b}'] {
            assert!(!py_isspace(c), "{c:?}");
        }
        assert!(!'\x1f'.is_whitespace(), "the case Rust's predicate misses");
    }

    /// CPython 3.12 `repr()`: quote choice, the four named escapes, and
    /// `\xNN`/`\uNNNN` for every non-`str.isprintable()` character —
    /// probed (`repr('term\xa0')` → `'term\xa0'`, `repr('a\u3000b')` →
    /// `'a\u3000b'`, `repr('a\x85b')` → `'a\x85b'`; panel fix round F).
    #[test]
    fn py_repr_str_quotes_and_escapes_like_cpython() {
        assert_eq!(py_repr_str("a"), "'a'");
        assert_eq!(py_repr_str("it's"), "\"it's\"");
        assert_eq!(py_repr_str("say \"hi\""), "'say \"hi\"'");
        assert_eq!(py_repr_str("both ' and \""), "'both \\' and \"'");
        assert_eq!(py_repr_str("a\\b"), "'a\\\\b'");
        assert_eq!(py_repr_str("a\nb"), "'a\\nb'");
        assert_eq!(py_repr_str("a\tb\rc"), "'a\\tb\\rc'");
        assert_eq!(py_repr_str("term\u{a0}"), "'term\\xa0'");
        assert_eq!(py_repr_str("foo\u{a0}bar"), "'foo\\xa0bar'");
        assert_eq!(py_repr_str("a\u{3000}b"), "'a\\u3000b'");
        assert_eq!(py_repr_str("a\u{85}b"), "'a\\x85b'");
        assert_eq!(py_repr_str("a\x1fb\x7f"), "'a\\x1fb\\x7f'");
        assert_eq!(py_repr_str("a\u{2028}b"), "'a\\u2028b'");
        assert_eq!(py_repr_str("é ü"), "'é ü'", "printable non-ASCII stays raw");
    }

    /// [`simplify_verbatim`] over Windows-shaped literals, which is the
    /// only way to exercise the rule off Windows (`canonicalize` there
    /// hands back exactly these shapes). The plain spelling is what
    /// `pathlib.Path.resolve()` returns, and the only one Windows
    /// normalizes `/` and `..` inside.
    #[test]
    fn the_verbatim_prefix_is_stripped_back_to_the_python_spelling() {
        let simplify = |text: &str| {
            simplify_verbatim(PathBuf::from(text))
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(simplify(r"\\?\C:\Users\me\docs"), r"C:\Users\me\docs");
        assert_eq!(simplify(r"\\?\c:\x"), r"c:\x");
        assert_eq!(simplify(r"\\?\UNC\server\share\doc"), r"\\server\share\doc");
        // Not a drive path: no plain spelling exists, so it keeps the
        // prefix rather than becoming unopenable.
        assert_eq!(simplify(r"\\?\Volume{9f8a}\x"), r"\\?\Volume{9f8a}\x");
        // Past MAX_PATH the prefix is what makes the path openable.
        let long = format!(r"\\?\C:\{}", "a".repeat(300));
        assert_eq!(simplify(&long), long);
        // Everything else — every POSIX path included — is untouched.
        assert_eq!(simplify("/tmp/x/y"), "/tmp/x/y");
        assert_eq!(simplify(r"C:\already\plain"), r"C:\already\plain");
        assert_eq!(simplify(r"\\server\share"), r"\\server\share");
    }

    /// Sphinx `.resolve()`s the joined path, so `..` walks up from a
    /// symlink's TARGET, not from the link's own parent. The lexical
    /// collapse [`relfn2path`] keeps for §Scope-8 display spellings gets
    /// this wrong, which is why the read goes through
    /// [`relfn2path_io`].
    ///
    // oracle: sphinx/environment/__init__.py:475
    //   `abs_fn = self.srcdir.joinpath(doc_dir, file_name).resolve()`
    //   (probed: with BASE/src/link -> BASE/ext, a literalinclude of
    //   `link/../secret.txt` reads BASE/ext/../secret.txt = BASE/secret.txt,
    //   not BASE/src/secret.txt).
    #[test]
    fn relfn2path_io_walks_up_from_the_symlink_target() {
        let base = tempfile::tempdir().unwrap();
        let base = canonicalize_simplified(base.path()).unwrap();
        let srcdir = base.join("src");
        std::fs::create_dir_all(srcdir.join("real")).unwrap();
        std::fs::create_dir_all(base.join("ext/inner")).unwrap();
        std::fs::write(base.join("ext/sibling.txt"), "OUTSIDE\n").unwrap();
        std::fs::write(srcdir.join("sibling.txt"), "INSIDE\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(base.join("ext/inner"), srcdir.join("link")).unwrap();

        // Lexically, `link/../sibling.txt` is `sibling.txt` under srcdir.
        assert_eq!(
            relfn2path("link/../sibling.txt", "index", &srcdir),
            srcdir.join("sibling.txt")
        );
        // Resolved, `link` is `<base>/ext/inner`, so `..` lands in
        // `<base>/ext` — a different file entirely.
        #[cfg(unix)]
        assert_eq!(
            relfn2path_io("link/../sibling.txt", "index", &srcdir),
            base.join("ext/sibling.txt")
        );

        // A path with no symlink in it is unchanged by the resolve, and a
        // target that does not exist yet still normalizes.
        assert_eq!(
            relfn2path_io("real/../sibling.txt", "index", &srcdir),
            srcdir.join("sibling.txt")
        );
        assert_eq!(
            relfn2path_io("real/../nothere.txt", "index", &srcdir),
            srcdir.join("nothere.txt")
        );
        assert_eq!(
            relfn2path_io("/sibling.txt", "sub/page", &srcdir),
            srcdir.join("sibling.txt")
        );
    }
}
