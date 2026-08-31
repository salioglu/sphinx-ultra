//! `objects.inv` reader/writer — mirrors `sphinx.util.inventory` (Sphinx 9.1.0).
//!
//! The file is framed as: one `\n`-terminated ASCII header line naming the
//! format version, followed by version-specific content. Version 2 (the only
//! format any Sphinx release still *writes*, though 1 is still read) packs
//! three more header lines and then a raw zlib-compressed byte tail — never
//! text, never line-split, never re-encoded — that decompresses to one text
//! record per object. Every byte-framing decision below (`partition`/`split`
//! at `\n`, blind column-11 header slices, the exact regex, `$`-suffix
//! expansion happening *before* the posixpath join, dispname `-` stored
//! verbatim) mirrors `sphinx/util/inventory.py` `InventoryFile.loads` /
//! `_loads_v1` / `_loads_v2` / `dump` byte-for-byte; see
//! docs/superpowers/plans/2026-08-31-m2-wave4-research-spec-inventory-intersphinx.md
//! §1 for the file:line citations this was built against.
//!
//! The previous reader here decoded the whole file with
//! `String::from_utf8_lossy` and iterated `.lines()` over it — which mangles
//! the raw zlib payload (lossy UTF-8 replacement + line-splitting on bytes
//! that were never text) on every real-world inventory. This rewrite never
//! treats the compressed tail as anything but a byte slice until *after*
//! `zlib::decompress` has run.

use anyhow::{Context, Result};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use log::{debug, info};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::Path;
use tokio::fs;

/// Inventory item representing a single object in the documentation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct InventoryItem {
    pub project_name: String,
    pub project_version: String,
    pub uri: String,
    pub display_name: String,
}

impl InventoryItem {
    pub fn new(
        project_name: String,
        project_version: String,
        uri: String,
        display_name: String,
    ) -> Self {
        Self {
            project_name,
            project_version,
            uri,
            display_name,
        }
    }
}

/// In-memory inventory data structure
#[derive(Debug, Clone, Default)]
pub struct Inventory {
    pub data: HashMap<String, HashMap<String, InventoryItem>>,
}

impl Inventory {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    /// Insert an item into the inventory
    pub fn insert(&mut self, obj_type: String, name: String, item: InventoryItem) {
        self.data.entry(obj_type).or_default().insert(name, item);
    }

    /// Get an item from the inventory
    pub fn get(&self, obj_type: &str, name: &str) -> Option<&InventoryItem> {
        self.data.get(obj_type)?.get(name)
    }

    /// Check if an item exists in the inventory
    pub fn contains(&self, obj_type: &str, name: &str) -> bool {
        self.data
            .get(obj_type)
            .is_some_and(|objects| objects.contains_key(name))
    }
}

/// One object record to write, in the shape `domain.get_objects()` yields:
/// `name` is the fully-qualified object name (`fullname` in Sphinx), `objtype`
/// is the bare type within its domain (no `domain:` prefix — the domain name
/// is supplied separately by [`InventoryFile::dump`]'s `domains` argument).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvObject {
    pub name: String,
    pub objtype: String,
    pub priority: i32,
    pub docname: String,
    pub anchor: String,
    pub dispname: String,
}

/// posixpath.join(uri, location) semantics: an absolute `location` (leading
/// `/`) replaces `uri` outright; otherwise `location` is appended, with a
/// `/` inserted only if `uri` is non-empty and doesn't already end in one.
/// (`sphinx/util/inventory.py:79,157` — `location = posixpath.join(uri, location)`.)
pub fn posix_join(uri: &str, location: &str) -> String {
    if location.starts_with('/') {
        location.to_string()
    } else if uri.is_empty() || uri.ends_with('/') {
        format!("{uri}{location}")
    } else {
        format!("{uri}/{location}")
    }
}

lazy_static::lazy_static! {
    /// `r'(.+?)\s+(\S+)\s+(-?\d+)\s+?(\S*)\s+(.*)'`, anchored at the start to
    /// match Python's `re.match` (which only requires the match to *start*
    /// at position 0, not consume the whole line) — `inventory.py:115-119`.
    static ref V2_LINE_RE: regex::Regex =
        regex::Regex::new(r"^(.+?)\s+(\S+)\s+(-?\d+)\s+?(\S*)\s+(.*)").unwrap();
    /// Header-field whitespace-run collapse: `re.sub('\s+', ' ', s)` — applied
    /// only to the Project/Version header values, never to entry lines
    /// (`inventory.py:178-179`).
    static ref WHITESPACE_RUN_RE: regex::Regex = regex::Regex::new(r"\s+").unwrap();
}

/// Inventory file handler - mirrors Sphinx's InventoryFile class
pub struct InventoryFile;

impl InventoryFile {
    /// Load inventory from bytes (mirrors Sphinx's `InventoryFile.loads`).
    ///
    /// Operates on raw bytes throughout the header framing and the
    /// version-2 zlib tail — the compressed payload is never decoded as
    /// text, never line-split, until *after* `zlib::decompress` succeeds.
    pub fn loads(content: &[u8], uri: &str) -> Result<Inventory> {
        let (format_line, rest) = partition_bytes(content, b'\n');
        let format_line = rstrip_bytes(format_line);

        if format_line == b"# Sphinx inventory version 2" {
            Self::loads_v2(rest, uri)
        } else if format_line == b"# Sphinx inventory version 1" {
            Self::loads_v1(rest, uri)
        } else if let Some(unknown_version_bytes) =
            format_line.strip_prefix(b"# Sphinx inventory version ")
        {
            let unknown_version = String::from_utf8(unknown_version_bytes.to_vec())
                .context("inventory header version suffix is not valid UTF-8")?;
            anyhow::bail!(
                "unknown or unsupported inventory version: {}",
                python_repr_str(&unknown_version)
            );
        } else {
            let line = String::from_utf8(format_line.to_vec())
                .context("inventory header line is not valid UTF-8")?;
            anyhow::bail!("invalid inventory header: {}", line);
        }
    }

    /// Load inventory from file
    pub async fn load<P: AsRef<Path>>(filename: P, uri: &str) -> Result<Inventory> {
        let content = fs::read(filename.as_ref()).await.with_context(|| {
            format!(
                "Failed to read inventory file: {}",
                filename.as_ref().display()
            )
        })?;

        Self::loads(&content, uri)
    }

    /// Load inventory from version 1 format (`inventory.py:70-93`).
    ///
    /// `content` is everything after the format line's `\n` (still raw
    /// bytes); v1 is plain text, so the *whole* remainder is UTF-8-decoded
    /// up front, then split with Python `str.splitlines()` semantics.
    fn loads_v1(content: &[u8], uri: &str) -> Result<Inventory> {
        let text =
            String::from_utf8(content.to_vec()).context("v1 inventory body is not valid UTF-8")?;
        let lines = python_str_splitlines(&text);

        if lines.len() < 2 {
            anyhow::bail!("invalid inventory header: missing project name or version");
        }

        let mut inv = Inventory::new();
        let projname = str_slice_from_char(lines[0].trim_end(), 11).to_string();
        let version = str_slice_from_char(lines[1].trim_end(), 11).to_string();

        for line in &lines[2..] {
            let fields = python_split_none_maxsplit(line.trim_end(), 2);
            if fields.len() != 3 {
                anyhow::bail!(
                    "invalid inventory v1 entry (expected `name type location`): {}",
                    line
                );
            }
            let (name, item_type, location) = (fields[0], fields[1], fields[2]);
            let mut location = posix_join(uri, location);

            // v1 did not add anchors to the location; do it here as plain
            // string concatenation, same as Sphinx (inventory.py:80-86) —
            // note this happens *after* the join, unlike v2's $-expansion.
            let domain_type = if item_type == "mod" {
                location.push_str("#module-");
                location.push_str(name);
                "py:module".to_string()
            } else {
                location.push('#');
                location.push_str(name);
                format!("py:{item_type}")
            };

            let item =
                InventoryItem::new(projname.clone(), version.clone(), location, "-".to_string());
            inv.insert(domain_type, name.to_string(), item);
        }

        Ok(inv)
    }

    /// Load inventory from version 2 format (`inventory.py:96-172`).
    ///
    /// `content` is everything after the format line's `\n`, still raw
    /// bytes. Framing (`split(b'\n', maxsplit=3)`), the column-11 header
    /// slices, and the `zlib` substring check all operate on bytes; only
    /// the decompressed entry payload is ever treated as text.
    fn loads_v2(content: &[u8], uri: &str) -> Result<Inventory> {
        let parts = splitn_bytes(content, b'\n', 4);
        if parts.len() != 4 {
            anyhow::bail!("invalid inventory header: missing project name or version");
        }
        let (line_1, line_2, check_line, compressed) = (parts[0], parts[1], parts[2], parts[3]);

        // Blind slice at byte column 11 (`len("# Project: ")`), no prefix
        // validation — inventory.py:103-104.
        let projname = String::from_utf8(bytes_slice_from(rstrip_bytes(line_1), 11).to_vec())
            .context("inventory Project header is not valid UTF-8")?;
        let version = String::from_utf8(bytes_slice_from(rstrip_bytes(line_2), 11).to_vec())
            .context("inventory Version header is not valid UTF-8")?;

        // check_line is used as-is: NOT rstripped (inventory.py:108-110).
        if !contains_bytes(check_line, b"zlib") {
            let check_line_text = String::from_utf8(check_line.to_vec())
                .context("inventory compression-check line is not valid UTF-8")?;
            anyhow::bail!(
                "invalid inventory header (not compressed): {}",
                check_line_text
            );
        }

        let decompressed = decompress_zlib(compressed)?;
        let decompressed_text = String::from_utf8(decompressed)
            .context("decompressed inventory payload is not valid UTF-8")?;

        let mut inv = Inventory::new();
        // definition (lowercased) -> (prio, location, dispname) as parsed,
        // BEFORE $-expansion/posix_join — inventory.py:106,140.
        let mut potential_ambiguities: HashMap<String, (String, String, String)> = HashMap::new();
        let mut actual_ambiguities: HashSet<String> = HashSet::new();

        for line in python_str_splitlines(&decompressed_text) {
            let trimmed = line.trim_end();
            let Some(caps) = V2_LINE_RE.captures(trimmed) else {
                continue;
            };
            let name = caps.get(1).unwrap().as_str();
            let type_ = caps.get(2).unwrap().as_str();
            let prio = caps.get(3).unwrap().as_str();
            let mut location = caps.get(4).unwrap().as_str().to_string();
            let dispname = caps.get(5).unwrap().as_str().to_string();

            if !type_.contains(':') {
                // Deliberately a plain string check, not part of the regex,
                // to avoid ReDoS (GH sphinx-doc/sphinx#8175).
                continue;
            }
            if type_ == "py:module" && inv.contains(type_, name) {
                // Sphinx <=1.1 double-emitted py:module entries; first wins.
                continue;
            }

            if type_ == "std:label" || type_ == "std:term" {
                let definition = format!("{type_}:{name}");
                let content_key = (prio.to_string(), location.clone(), dispname.clone());
                let lowercase_definition = definition.to_lowercase();
                match potential_ambiguities.get(&lowercase_definition) {
                    Some(existing) if existing == &content_key => {
                        debug!(
                            "inventory <{}> contains duplicate definitions of {}",
                            uri, definition
                        );
                    }
                    Some(_) => {
                        actual_ambiguities.insert(definition);
                    }
                    None => {
                        potential_ambiguities.insert(lowercase_definition, content_key);
                    }
                }
            }

            if let Some(prefix) = location.strip_suffix('$') {
                location = format!("{prefix}{name}");
            }
            let joined = posix_join(uri, &location);

            let item = InventoryItem::new(projname.clone(), version.clone(), joined, dispname);
            inv.insert(type_.to_string(), name.to_string(), item);
        }

        for ambiguity in &actual_ambiguities {
            info!(
                "inventory <{}> contains multiple definitions for {}",
                uri, ambiguity
            );
        }

        Ok(inv)
    }

    /// Write inventory to `path` in Sphinx's version-2 format
    /// (mirrors `InventoryFile.dump`, `inventory.py:174-207`).
    ///
    /// Decoupled from `BuildEnvironment`/`Builder`: callers supply the
    /// already-collected per-domain object lists and a `get_target_uri`
    /// closure (`Builder.get_target_uri(docname)` in Sphinx) instead of
    /// live env/builder references.
    ///
    /// `domains` need not be pre-sorted — this sorts domains alphabetically
    /// by name and, within each domain, sorts its objects by
    /// `(name, dispname, objtype, docname, anchor, priority)`, mirroring
    /// `env.domains.sorted()` + `sorted(domain.get_objects())`
    /// (`inventory.py:194-196`).
    pub async fn dump<P: AsRef<Path>>(
        path: P,
        project: &str,
        version: &str,
        domains: &[(&str, Vec<InvObject>)],
        get_target_uri: impl Fn(&str) -> String,
    ) -> Result<()> {
        let header = format!(
            "# Sphinx inventory version 2\n\
             # Project: {}\n\
             # Version: {}\n\
             # The remainder of this file is compressed using zlib.\n",
            Self::escape_string(project),
            Self::escape_string(version),
        );

        let mut sorted_domains: Vec<&(&str, Vec<InvObject>)> = domains.iter().collect();
        sorted_domains.sort_by_key(|(name, _)| *name);

        let mut body = Vec::new();
        for (domain_name, objects) in sorted_domains {
            let mut objects: Vec<&InvObject> = objects.iter().collect();
            objects.sort_by(|a, b| {
                a.name
                    .cmp(&b.name)
                    .then_with(|| a.dispname.cmp(&b.dispname))
                    .then_with(|| a.objtype.cmp(&b.objtype))
                    .then_with(|| a.docname.cmp(&b.docname))
                    .then_with(|| a.anchor.cmp(&b.anchor))
                    .then_with(|| a.priority.cmp(&b.priority))
            });

            for obj in objects {
                // `if anchor.endswith(fullname): anchor = anchor.removesuffix(fullname) + '$'`
                // (inventory.py:197-199) — up to ~25% size saving.
                let anchor = match obj.anchor.strip_suffix(obj.name.as_str()) {
                    Some(prefix) => format!("{prefix}$"),
                    None => obj.anchor.clone(),
                };

                // `#` is part of the URI, added before the (possibly
                // $-abbreviated) anchor, and only when anchor is non-empty
                // (inventory.py:200-202) — the old writer's missing-`#` bug.
                let mut uri = get_target_uri(&obj.docname);
                if !anchor.is_empty() {
                    uri.push('#');
                    uri.push_str(&anchor);
                }

                let dispname: &str = if obj.dispname == obj.name {
                    "-"
                } else {
                    obj.dispname.as_str()
                };

                let line = format!(
                    "{} {}:{} {} {} {}\n",
                    obj.name, domain_name, obj.objtype, obj.priority, uri, dispname
                );
                body.extend_from_slice(line.as_bytes());
            }
        }

        // One-shot compression of the whole body is byte-equivalent to
        // Sphinx's per-entry `compressor.compress()` calls with a single
        // final `flush()` and no intermediate flushes (inventory.py:206-207)
        // — but see the module doc: flate2's backend never produces
        // CPython-zlib-identical *compressed* bytes regardless, so
        // byte-correctness is defined on the decompressed payload only.
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(9));
        encoder
            .write_all(&body)
            .context("failed to compress inventory body")?;
        let compressed = encoder
            .finish()
            .context("failed to finalize inventory zlib stream")?;

        let mut content = header.into_bytes();
        content.extend_from_slice(&compressed);

        fs::write(path, content)
            .await
            .context("Failed to write inventory file")?;

        Ok(())
    }

    /// Escape a header field value: whitespace runs collapsed to a single
    /// space (`re.sub('\s+', ' ', s)`, `inventory.py:178-179`). Applies only
    /// to the Project/Version header fields — entry lines are not escaped.
    fn escape_string(s: &str) -> String {
        WHITESPACE_RUN_RE.replace_all(s, " ").to_string()
    }
}

/// `bytes.partition(sep)` — always returns `(head, tail)`; if `sep` isn't
/// found, `head` is the whole input and `tail` is empty (Python's `partition`
/// also returns an empty *separator* in that case, which callers here never
/// need to distinguish from "found at the very end").
fn partition_bytes(data: &[u8], sep: u8) -> (&[u8], &[u8]) {
    match data.iter().position(|&b| b == sep) {
        Some(pos) => (&data[..pos], &data[pos + 1..]),
        None => (data, &[]),
    }
}

/// `bytes.split(sep, maxsplit=n-1)` — at most `n` parts; the last part keeps
/// any further `sep` bytes un-split. Returns fewer than `n` parts if there
/// aren't enough separators, exactly like Python.
fn splitn_bytes(data: &[u8], sep: u8, n: usize) -> Vec<&[u8]> {
    let mut parts = Vec::with_capacity(n);
    let mut rest = data;
    while parts.len() + 1 < n {
        match rest.iter().position(|&b| b == sep) {
            Some(pos) => {
                parts.push(&rest[..pos]);
                rest = &rest[pos + 1..];
            }
            None => break,
        }
    }
    parts.push(rest);
    parts
}

/// `bytes.rstrip()` (no argument): strips trailing ASCII whitespace
/// (space, \t, \n, \r, \x0b, \x0c).
fn rstrip_bytes(data: &[u8]) -> &[u8] {
    let mut end = data.len();
    while end > 0 && matches!(data[end - 1], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
        end -= 1;
    }
    &data[..end]
}

/// `data[start:]` on a `bytes` object: never panics/errors even if `start`
/// is past the end (returns empty), matching Python's forgiving slicing.
fn bytes_slice_from(data: &[u8], start: usize) -> &[u8] {
    if start >= data.len() {
        &[]
    } else {
        &data[start..]
    }
}

/// `s[start:]` on a `str`, where `start` counts Unicode *characters* (not
/// bytes) — matching Python's `str` slicing, which is always
/// character-indexed and never panics on a short string.
fn str_slice_from_char(s: &str, start: usize) -> &str {
    match s.char_indices().nth(start) {
        Some((byte_idx, _)) => &s[byte_idx..],
        None => "",
    }
}

/// `sep in data` for byte slices (Python's `in` on `bytes`).
fn contains_bytes(data: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    data.windows(needle.len()).any(|w| w == needle)
}

/// `str.splitlines()`: splits on `\n`, `\r`, `\r\n` (as one boundary), and
/// the other line-boundary characters Python recognizes (`\v`, `\f`,
/// `\x1c`-`\x1e`, `\x85`, U+2028, U+2029). Unlike `str::split`, a trailing
/// boundary produces no trailing empty element.
fn python_str_splitlines(s: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut chars = s.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        let is_boundary = matches!(
            ch,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if is_boundary {
            lines.push(&s[start..idx]);
            let mut end = idx + ch.len_utf8();
            if ch == '\r' {
                if let Some(&(_, '\n')) = chars.peek() {
                    let (nidx, nch) = chars.next().unwrap();
                    end = nidx + nch.len_utf8();
                }
            }
            start = end;
        }
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

/// `s.split(None, maxsplit=n)`: skips leading whitespace, collects up to `n`
/// whitespace-delimited tokens, then the final element is whatever remains
/// (its own leading whitespace consumed by the split, but not further
/// trimmed). An all-whitespace or empty `s` yields an empty `Vec`.
fn python_split_none_maxsplit(s: &str, maxsplit: usize) -> Vec<&str> {
    let mut result = Vec::new();
    let mut rest = s;
    loop {
        let trimmed = rest.trim_start();
        if trimmed.is_empty() {
            break;
        }
        if result.len() == maxsplit {
            result.push(trimmed);
            break;
        }
        match trimmed.find(char::is_whitespace) {
            Some(idx) => {
                result.push(&trimmed[..idx]);
                rest = &trimmed[idx..];
            }
            None => {
                result.push(trimmed);
                rest = "";
            }
        }
    }
    result
}

/// A reasonable approximation of Python's `repr()` for `str`, sufficient for
/// the one place it's needed (`{unknown_version!r}` in the unsupported-
/// inventory-version error, `inventory.py:59-61`): a realistic version
/// suffix is plain ASCII. Quotes with `'` unless the string contains a `'`
/// and no `"`, in which case it quotes with `"`; escapes backslashes, the
/// chosen quote character, and ASCII control characters as `\xNN`. Does NOT
/// replicate Python's full non-ASCII-category escaping (`\uXXXX` for exotic
/// Unicode control/separator characters) — out of scope for this field.
fn python_repr_str(s: &str) -> String {
    let has_single = s.contains('\'');
    let has_double = s.contains('"');
    let quote = if has_single && !has_double { '"' } else { '\'' };

    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn decompress_zlib(data: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(data);
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .context("failed to decompress inventory zlib payload")?;
    Ok(decompressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inventory_item_creation() {
        let item = InventoryItem::new(
            "test_project".to_string(),
            "1.0".to_string(),
            "http://example.com/test.html".to_string(),
            "Test Item".to_string(),
        );

        assert_eq!(item.project_name, "test_project");
        assert_eq!(item.project_version, "1.0");
        assert_eq!(item.uri, "http://example.com/test.html");
        assert_eq!(item.display_name, "Test Item");
    }

    #[test]
    fn test_inventory_operations() {
        let mut inv = Inventory::new();

        let item = InventoryItem::new(
            "test".to_string(),
            "1.0".to_string(),
            "test.html".to_string(),
            "Test".to_string(),
        );

        inv.insert(
            "py:function".to_string(),
            "test_func".to_string(),
            item.clone(),
        );

        assert!(inv.contains("py:function", "test_func"));
        assert_eq!(inv.get("py:function", "test_func"), Some(&item));
        assert!(!inv.contains("py:function", "nonexistent"));
    }

    #[test]
    fn test_escape_string() {
        assert_eq!(
            InventoryFile::escape_string("test   multiple   spaces"),
            "test multiple spaces"
        );
        assert_eq!(InventoryFile::escape_string("test\ttab"), "test tab");
        assert_eq!(
            InventoryFile::escape_string("test\nnewline"),
            "test newline"
        );
    }

    // -- posix_join: mirrors posixpath.join(uri, location), verified against
    // real `posixpath.join` output for every case below. --

    #[test]
    fn test_posix_join_inserts_separator() {
        assert_eq!(posix_join("/util", "foo.html"), "/util/foo.html");
    }

    #[test]
    fn test_posix_join_no_double_separator() {
        assert_eq!(posix_join("/util/", "foo.html"), "/util/foo.html");
    }

    #[test]
    fn test_posix_join_empty_location() {
        assert_eq!(posix_join("/util", ""), "/util/");
    }

    #[test]
    fn test_posix_join_empty_uri() {
        assert_eq!(posix_join("", "foo.html"), "foo.html");
    }

    #[test]
    fn test_posix_join_absolute_location_overrides_uri() {
        assert_eq!(posix_join("/util", "/abs/path.html"), "/abs/path.html");
    }

    #[test]
    fn test_posix_join_both_empty() {
        assert_eq!(posix_join("", ""), "");
    }

    #[test]
    fn test_posix_join_uri_with_scheme() {
        assert_eq!(
            posix_join("https://example.org/v1", "sub/x.html#y"),
            "https://example.org/v1/sub/x.html#y"
        );
    }

    // -- python_str_splitlines --

    #[test]
    fn test_splitlines_mixed_separators() {
        assert_eq!(
            python_str_splitlines("a\r\nb\rc\u{0b}d\u{0c}e"),
            vec!["a", "b", "c", "d", "e"]
        );
    }

    #[test]
    fn test_splitlines_no_trailing_empty() {
        assert_eq!(python_str_splitlines("a\nb\n"), vec!["a", "b"]);
    }

    #[test]
    fn test_splitlines_empty_string() {
        assert!(python_str_splitlines("").is_empty());
    }

    #[test]
    fn test_splitlines_lone_newline() {
        assert_eq!(python_str_splitlines("\n"), vec![""]);
    }

    #[test]
    fn test_splitlines_embedded_blank_line() {
        assert_eq!(python_str_splitlines("a\n\nb"), vec!["a", "", "b"]);
    }

    // -- python_split_none_maxsplit --

    #[test]
    fn test_split_none_maxsplit_collapses_runs() {
        assert_eq!(
            python_split_none_maxsplit("module   mod    foo.html", 2),
            vec!["module", "mod", "foo.html"]
        );
    }

    #[test]
    fn test_split_none_maxsplit_remainder_keeps_internal_whitespace() {
        assert_eq!(
            python_split_none_maxsplit("a b c d e", 2),
            vec!["a", "b", "c d e"]
        );
    }

    #[test]
    fn test_split_none_maxsplit_empty() {
        assert!(python_split_none_maxsplit("", 2).is_empty());
        assert!(python_split_none_maxsplit("   ", 2).is_empty());
    }

    #[test]
    fn test_split_none_maxsplit_too_few_tokens() {
        assert_eq!(python_split_none_maxsplit("onlyone", 2), vec!["onlyone"]);
    }

    // -- python_repr_str --

    #[test]
    fn test_python_repr_str_plain() {
        assert_eq!(python_repr_str("5"), "'5'");
    }

    #[test]
    fn test_python_repr_str_prefers_single_quotes() {
        assert_eq!(python_repr_str("2.5-beta"), "'2.5-beta'");
    }

    #[test]
    fn test_python_repr_str_switches_to_double_quotes() {
        assert_eq!(python_repr_str("it's"), "\"it's\"");
    }

    // -- v2 line regex: no match on a genuinely non-conforming line (must
    // be silently skippable by the caller, never fall back to a looser
    // split) --

    #[test]
    fn test_v2_line_regex_no_match_on_garbage() {
        // No `-?\d+` priority field anywhere in this line, so no match can
        // exist at any start position (with or without the `^` anchor).
        assert!(V2_LINE_RE
            .captures("not a valid entry line at all")
            .is_none());
    }

    #[test]
    fn test_v2_line_regex_captures_five_groups() {
        let caps = V2_LINE_RE
            .captures("a term including:colon std:term -1 glossary.html#term -")
            .unwrap();
        assert_eq!(&caps[1], "a term including:colon");
        assert_eq!(&caps[2], "std:term");
        assert_eq!(&caps[3], "-1");
        assert_eq!(&caps[4], "glossary.html#term");
        assert_eq!(&caps[5], "-");
    }
}
