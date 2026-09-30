//! Differential test: sphinx-ultra's HTML OUTPUT TREE vs the PAGE-LEVEL HTML
//! ORACLE -- the files a real `sphinx-build -b html` / `-b dirhtml` 9.1.0
//! writes for the committed project corpus (M2 wave 5, design decision 7).
//!
//! Regenerate the fixtures (manual, never in CI):
//!     PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' \
//!         --with 'docutils==0.22.4' --with 'pygments==2.21.0' \
//!         --with 'jinja2==3.1.6' --with 'markupsafe==3.0.3' \
//!         --with 'alabaster==1.0.0' --with 'imagesize==2.0.1' \
//!         --with 'snowballstemmer==3.1.1' --with 'babel==2.18.0' \
//!         --with 'sphinxcontrib-htmlhelp==2.1.0' \
//!         --with 'sphinxcontrib-qthelp==2.0.0' \
//!         python tools/gen_html_fixture.py
//!
//! The fixture is one JSON file per corpus family
//! (`tests/fixtures/html_differential_<family>.json`, see [`FIXTURES`]); the
//! generator's module docstring is the contract for every recorded key and
//! for the three normalization tokens.
//!
//! # What is compared
//!
//! Each project is materialized into a tempdir (`source/` with `conf.py`,
//! the documents, text and base64-decoded binary members), configured the
//! way `sphinx-build` would be (the crate's own `conf.py` loader, then every
//! fixture `conf` key as a `-D` override through
//! [`BuildConfig::apply_override`], plus `builder` for the dirhtml projects),
//! built with [`SphinxBuilder::build`] into `build/`, and the output tree on
//! disk is compared key by key -- one test per key, each with its own strict,
//! self-cleaning exemption table:
//!
//! | test | key | table |
//! |---|---|---|
//! | [`output_file_sets_match_oracle`] | the set of relpaths | [`KNOWN_FILE_SET_GAPS`] (+ the [`OUR_ONLY_PREFIXES`] / [`M3_FILES`] allowlists) |
//! | [`pages_match_oracle`] | every page, byte for byte after normalization | [`KNOWN_PAGE_GAPS`] |
//! | [`assets_match_oracle`] | `_static/`, `_images/`, `_downloads/`, extra files, by sha256 | [`KNOWN_ASSET_GAPS`] |
//! | [`sources_match_oracle`] | `_sources/*` | [`KNOWN_SOURCE_GAPS`] |
//! | [`buildinfo_matches_oracle`] | `.buildinfo` | [`KNOWN_BUILDINFO_GAPS`] |
//! | [`inventories_match_oracle`] | `objects.inv` header + decompressed payload | [`KNOWN_INVENTORY_GAPS`] |
//! | [`warnings_match_oracle`] | the warning records | [`KNOWN_WARNING_GAPS`] |
//!
//! These seven are `#[ignore]`d until the HTML builder lands (M2 wave 5 T6);
//! un-ignoring them is T6's job, filling the tables honestly as it goes. The
//! fixture-loading, self-consistency, helper and exemption-arithmetic tests
//! below run now.
//!
//! # Exemption discipline
//!
//! Every table entry is `(project, path, reason)` (per-project keys:
//! `(project, reason)`), where `project` and `path` may be `"*"`. An entry
//! COVERS the compared pairs it matches and is checked **strictly**: every
//! pair it covers must still diverge (one pair that now matches the oracle
//! fails the test -- narrow the entry or delete it), it must cover at least
//! one compared pair (an entry naming nothing fails), and no pair may be
//! covered by two entries. [`exemption_arithmetic_matches_the_documented_numbers`]
//! ties the table sizes to documented counts.
//!
//! # Normalizations (identical on both sides)
//!
//! * the srcdir -> `<project>`, the outdir -> `<outdir>` (the oracle's
//!   generator already did its side; [`normalize_paths`] does ours, with the
//!   same separator handling as tests/env_differential.rs);
//! * the footer's generator credit -> `@@generator-credit@@`: design
//!   decision 5 makes sphinx-ultra render an HONEST credit instead of
//!   "Created using Sphinx"; the oracle replaced Sphinx's, and
//!   [`OUR_GENERATOR_CREDITS`] lists ours (T6 fills it with the exact
//!   wording it chooses);
//! * §Scope-8 ([`canon_scope8`]): `<project>/` collapses to the
//!   srcdir-relative spelling on both sides (keep_warnings pages and
//!   warnings print source paths);
//! * `?v=<crc32>` cache-busters are masked ONLY for `_static` files whose
//!   bytes already differ from the oracle's ([`mask_checksums`]) -- that
//!   difference is reported by [`assets_match_oracle`] once instead of on
//!   every page; for a byte-identical static file the checksum must match.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

use sphinx_ultra::{BuildConfig, SphinxBuilder};

// ---------------------------------------------------------------------------
// Fixture model
// ---------------------------------------------------------------------------

/// The fixture files, one per corpus family, in generation order.
const FIXTURES: &[(&str, &str)] = &[
    (
        "structural",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/html_differential_structural.json"
        )),
    ),
    (
        "nodes",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/html_differential_nodes.json"
        )),
    ),
    (
        "toctree",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/html_differential_toctree.json"
        )),
    ),
    (
        "indices",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/html_differential_indices.json"
        )),
    ),
    (
        "config",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/html_differential_config.json"
        )),
    ),
    (
        "dirhtml",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/html_differential_dirhtml.json"
        )),
    ),
    (
        "highlight",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/html_differential_highlight.json"
        )),
    ),
    (
        "smartquotes",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/html_differential_smartquotes.json"
        )),
    ),
    (
        "alabaster",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/html_differential_alabaster.json"
        )),
    ),
    (
        "themes",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/html_differential_themes.json"
        )),
    ),
];

/// The generator's schema version (`SCHEMA_VERSION` in
/// tools/gen_html_fixture.py). Bump both together when a key's meaning
/// changes; adding a key is extend-only and needs no bump.
const SCHEMA_VERSION: u32 = 1;

/// Every package that reaches a page, a static file or `.buildinfo`, pinned
/// by the regeneration command above and asserted by the generator.
const EXPECTED_PINS: &[(&str, &str)] = &[
    ("alabaster", "1.0.0"),
    ("babel", "2.18.0"),
    ("docutils", "0.22.4"),
    ("imagesize", "2.0.1"),
    ("jinja2", "3.1.6"),
    ("markupsafe", "3.0.3"),
    ("pygments", "2.21.0"),
    ("snowballstemmer", "3.1.1"),
    ("sphinx", "9.1.0"),
    ("sphinxcontrib-htmlhelp", "2.1.0"),
    ("sphinxcontrib-qthelp", "2.0.0"),
];

/// The fixture's stand-in for a project's srcdir.
const PROJECT: &str = "<project>";
/// The fixture's stand-in for a project's outdir.
const OUTDIR: &str = "<outdir>";
/// The fixture's stand-in for the footer's generator credit.
const CREDIT: &str = "@@generator-credit@@";

/// `stable_hash(sorted(tags))` for the builder tags Sphinx adds
/// unconditionally (`Builder.__init__`): html -> {builder_html, format_html,
/// html}, dirhtml -> {builder_dirhtml, dirhtml, format_html, html}.
const HTML_TAGS_HASH: &str = "645f666f9bcd5a90fca523b33c5a78b7";
const DIRHTML_TAGS_HASH: &str = "d77d1c0d9ca2f4c8421862c7c5a0d620";

/// Builtin themes whose layout empties the footer block, so their pages
/// carry no credit (themes/epub/layout.html:13, themes/nonav/layout.html:14).
const CREDITLESS_THEMES: &[&str] = &["epub", "nonav"];

#[derive(serde::Deserialize)]
struct FixtureFile {
    schema_version: u32,
    generator: String,
    family: String,
    sphinx_version: String,
    docutils_version: String,
    pins: BTreeMap<String, String>,
    pillow: String,
    base_conf: serde_json::Value,
    tokens: Tokens,
    /// Every `rebuild='html'` option of a plain one-document project with
    /// the base configuration: name -> (Python repr, `stable_hash`).
    buildinfo_reference: BTreeMap<String, OptionValue>,
    /// sha256 -> lines of every templated/generated static file's text.
    texts: BTreeMap<String, Vec<String>>,
    projects: Vec<Project>,
}

#[derive(serde::Deserialize, PartialEq, Debug)]
struct Tokens {
    project: String,
    outdir: String,
    generator_credit: String,
}

#[derive(serde::Deserialize, PartialEq, Debug)]
struct OptionValue {
    repr: String,
    hash: String,
}

#[derive(serde::Deserialize)]
struct Project {
    name: String,
    family: String,
    /// `html` or `dirhtml`.
    builder: String,
    /// The `-D` overrides (base configuration merged with the project's
    /// own): scalars, comma-free string lists, or one-level dicts of scalars
    /// (applied key by key, `-D numfig_format.figure=...`).
    conf: serde_json::Value,
    /// The full conf.py the oracle built with, written into the srcdir.
    conf_py: String,
    /// Base configuration keys the project leaves at Sphinx's default.
    #[serde(default)]
    unset: Vec<String>,
    /// Process environment around the build (only SOURCE_DATE_EPOCH).
    #[serde(default)]
    env: BTreeMap<String, String>,
    files: BTreeMap<String, String>,
    #[serde(default)]
    data_files: BTreeMap<String, String>,
    /// relpath -> base64 of a binary member (real PNGs).
    #[serde(default)]
    binary_files: BTreeMap<String, String>,
    expect: Expect,
}

#[derive(serde::Deserialize)]
struct Expect {
    output_files: BTreeMap<String, OutputFile>,
    /// Every file written through `handle_page`, normalized, as lines.
    pages: BTreeMap<String, Vec<String>>,
    page_context: BTreeMap<String, PageContext>,
    sources: BTreeMap<String, String>,
    buildinfo: String,
    /// Options whose (repr, hash) differ from the file's
    /// `buildinfo_reference` -- localizes a config-hash mismatch.
    buildinfo_config: BTreeMap<String, OptionValue>,
    /// False when a hashed leaf is not a str/int/bool/None.
    buildinfo_modelable: bool,
    #[serde(default)]
    buildinfo_unmodelable_leaves: Vec<String>,
    inventory: InventoryExpect,
    warnings: Vec<String>,
}

#[derive(serde::Deserialize)]
struct OutputFile {
    kind: String,
    /// sha256 of the recorded bytes: normalized text for pages, raw bytes
    /// otherwise; absent for the M3 `searchindex.js`.
    #[serde(default)]
    sha256: Option<String>,
    /// Whether the text itself is recorded (pages, sources, `.buildinfo`,
    /// templated statics via `texts`).
    #[serde(default)]
    text: bool,
}

#[derive(serde::Deserialize)]
struct PageContext {
    pagename: String,
    template: String,
    /// What `handle_page` was handed (for `page.html`: title, body, toc,
    /// display_toc, prev, next, parents, rellinks, sourcename, meta,
    /// metatags, page_source_suffix, has_maths_elements).
    addctx: serde_json::Value,
    /// A subset of the merged `html-page-context` context.
    ctx: serde_json::Value,
}

#[derive(serde::Deserialize)]
struct InventoryExpect {
    header: Vec<String>,
    payload: String,
}

const OUTPUT_KINDS: &[&str] = &[
    "page",
    "source",
    "static",
    "image",
    "download",
    "extra",
    "buildinfo",
    "inventory",
    "searchindex",
];

/// Kinds compared by sha256 in [`assets_match_oracle`].
const ASSET_KINDS: &[&str] = &["static", "image", "download", "extra"];

fn fixture_files() -> &'static [FixtureFile] {
    static FILES: OnceLock<Vec<FixtureFile>> = OnceLock::new();
    FILES.get_or_init(|| {
        FIXTURES
            .iter()
            .map(|(family, raw)| {
                serde_json::from_str(raw).unwrap_or_else(|e| {
                    panic!("html_differential_{family}.json does not parse: {e}")
                })
            })
            .collect()
    })
}

fn all_projects() -> impl Iterator<Item = (&'static FixtureFile, &'static Project)> {
    fixture_files()
        .iter()
        .flat_map(|file| file.projects.iter().map(move |project| (file, project)))
}

/// The theme a project builds with: its `html_theme` override, else
/// Sphinx's default (`alabaster`, `sphinx/builders/html/__init__.py:1459`),
/// else whatever its conf.py sets (the fixture never sets it there).
fn theme_of(project: &Project) -> &str {
    project
        .conf
        .get("html_theme")
        .and_then(|value| value.as_str())
        .unwrap_or("alabaster")
}

// ---------------------------------------------------------------------------
// Small pure helpers: sha256, base64, unified diff
// ---------------------------------------------------------------------------

/// SHA-256 (FIPS 180-4), hex-encoded. Kept here rather than as a
/// dev-dependency: the fixture stores sha256 because that is what Python's
/// hashlib gives the generator, and nothing else in the crate needs it.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for block in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(value);
        }
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

/// Standard-alphabet, padded base64 (what Python's `base64.b64encode`
/// writes into `binary_files`).
fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    fn value(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(format!("length {} is not a multiple of 4", bytes.len()));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (index, quad) in bytes.chunks_exact(4).enumerate() {
        let last = index == bytes.len() / 4 - 1;
        let padding = quad.iter().rev().take_while(|b| **b == b'=').count();
        if padding > 2 || (padding > 0 && !last) {
            return Err("misplaced padding".to_string());
        }
        let mut acc = 0u32;
        for &byte in &quad[..4 - padding] {
            let v = value(byte).ok_or_else(|| format!("invalid byte {byte:#x}"))?;
            acc = (acc << 6) | v;
        }
        acc <<= 6 * padding as u32;
        let decoded = [(acc >> 16) as u8, (acc >> 8) as u8, acc as u8];
        out.extend_from_slice(&decoded[..3 - padding]);
    }
    Ok(out)
}

/// A compact unified diff (`---`/`+++`, `@@ -a,b +c,d @@` hunks, `context`
/// lines around each change), truncated to `max_lines` output lines.
///
/// Common prefix and suffix are trimmed first; the middle is diffed with an
/// LCS table, which is quadratic, so a middle larger than
/// [`DIFF_CELL_BUDGET`] cells falls back to a single hunk spanning the
/// whole differing middle.
fn unified_diff(expected: &str, actual: &str, context: usize, max_lines: usize) -> String {
    let old: Vec<&str> = expected.split('\n').collect();
    let new: Vec<&str> = actual.split('\n').collect();
    if old == new {
        return String::new();
    }
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old_mid = &old[prefix..old.len() - suffix];
    let new_mid = &new[prefix..new.len() - suffix];

    // (tag, old index, new index) edit script over the whole texts.
    #[derive(Clone, Copy, PartialEq)]
    enum Op {
        Same,
        Del,
        Add,
    }
    let mut script: Vec<(Op, usize, usize)> = (0..prefix).map(|i| (Op::Same, i, i)).collect();
    if old_mid.len().saturating_mul(new_mid.len()) <= DIFF_CELL_BUDGET {
        let (n, m) = (old_mid.len(), new_mid.len());
        let mut lcs = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * (m + 1) + j] = if old_mid[i] == new_mid[j] {
                    lcs[(i + 1) * (m + 1) + j + 1] + 1
                } else {
                    lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && old_mid[i] == new_mid[j] {
                script.push((Op::Same, prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if i < n && (j == m || lcs[(i + 1) * (m + 1) + j] >= lcs[i * (m + 1) + j + 1]) {
                // Deletions first on ties: `-old` then `+new`, diff(1) order.
                script.push((Op::Del, prefix + i, prefix + j));
                i += 1;
            } else {
                script.push((Op::Add, prefix + i, prefix + j));
                j += 1;
            }
        }
    } else {
        script.extend((0..old_mid.len()).map(|i| (Op::Del, prefix + i, prefix)));
        script.extend((0..new_mid.len()).map(|j| (Op::Add, prefix + old_mid.len(), prefix + j)));
    }
    script.extend((0..suffix).map(|k| (Op::Same, old.len() - suffix + k, new.len() - suffix + k)));

    // Group changes into hunks with `context` lines of padding.
    let changed: Vec<usize> = script
        .iter()
        .enumerate()
        .filter(|(_, (op, _, _))| *op != Op::Same)
        .map(|(index, _)| index)
        .collect();
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for &index in &changed {
        let start = index.saturating_sub(context);
        let end = (index + context + 1).min(script.len());
        match hunks.last_mut() {
            Some(last) if start <= last.1 => last.1 = end,
            _ => hunks.push((start, end)),
        }
    }
    let mut out = vec!["--- oracle".to_string(), "+++ ours".to_string()];
    for (start, end) in hunks {
        let slice = &script[start..end];
        let old_count = slice.iter().filter(|(op, _, _)| *op != Op::Add).count();
        let new_count = slice.iter().filter(|(op, _, _)| *op != Op::Del).count();
        let (_, old_start, new_start) = slice[0];
        out.push(format!(
            "@@ -{},{old_count} +{},{new_count} @@",
            old_start + 1,
            new_start + 1
        ));
        for &(op, i, j) in slice {
            out.push(match op {
                Op::Same => format!(" {}", old[i]),
                Op::Del => format!("-{}", old[i]),
                Op::Add => format!("+{}", new[j]),
            });
        }
    }
    if out.len() > max_lines {
        let hidden = out.len() - max_lines;
        out.truncate(max_lines);
        out.push(format!("... ({hidden} more diff lines)"));
    }
    out.join("\n")
}

/// Largest LCS table [`unified_diff`] builds (old x new middle lines).
const DIFF_CELL_BUDGET: usize = 4_000_000;

/// 1-based number of the first line where two texts differ.
fn first_differing_line(expected: &str, actual: &str) -> usize {
    let mut old = expected.split('\n');
    let mut new = actual.split('\n');
    let mut line = 1;
    loop {
        match (old.next(), new.next()) {
            (Some(a), Some(b)) if a == b => line += 1,
            (None, None) => return line,
            _ => return line,
        }
    }
}

/// Which part of a basic-lineage page a 1-based line falls into, judged by
/// the last structural marker at or before it in the ORACLE page.
fn page_region(oracle: &str, line: usize) -> &'static str {
    const MARKERS: &[(&str, &str)] = &[
        ("<head>", "head"),
        ("</head>", "relbar"),
        ("<div class=\"related\"", "relbar"),
        ("<div class=\"document\">", "document"),
        ("<div class=\"body\"", "body"),
        ("<div class=\"sphinxsidebar\"", "sidebar"),
        ("<div class=\"footer", "footer"),
    ];
    let mut region = "preamble";
    for text in oracle.split('\n').take(line) {
        for (marker, name) in MARKERS {
            if text.contains(marker) {
                region = name;
            }
        }
    }
    region
}

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

/// Rewrite `source_root`- and `output_root`-rooted paths in `text` to the
/// fixture's [`PROJECT`] / [`OUTDIR`] tokens, spelling what follows a token
/// with forward slashes (the tests/env_differential.rs rule: the oracle is
/// POSIX, and on Windows sphinx prints native separators too -- so the flip
/// is a property of the comparison; only placeholder-rooted segments are
/// touched, each ending at whitespace, a quote, `:`, `,`, `<` or `)`).
fn normalize_paths(text: &str, source_root: &str, output_root: &str) -> String {
    let mut roots = [(source_root, PROJECT), (output_root, OUTDIR)];
    // Longest first, so a root nested in the other cannot be half-replaced.
    roots.sort_by_key(|(root, _)| std::cmp::Reverse(root.len()));
    let mut replaced = text.to_string();
    for (root, token) in roots {
        replaced = replaced
            .replace(&root.replace('\\', r"\\"), token)
            .replace(root, token);
    }
    let mut out = String::with_capacity(replaced.len());
    let mut rest = replaced.as_str();
    while let Some((start, token)) = [PROJECT, OUTDIR]
        .iter()
        .filter_map(|token| rest.find(token).map(|at| (at, *token)))
        .min()
    {
        out.push_str(&rest[..start]);
        out.push_str(token);
        let tail = &rest[start + token.len()..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ':' | '\'' | '"' | ',' | '<' | ')'))
            .unwrap_or(tail.len());
        out.push_str(&tail[..end].replace(r"\\", "/").replace('\\', "/"));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// §Scope-8, as in tests/env_differential.rs: collapse `<project>/` so the
/// canonical in-srcdir spelling is srcdir-relative, on BOTH sides. Only
/// prefix presence can be masked; a difference below the srcdir still
/// diverges.
fn canon_scope8(text: &str) -> String {
    debug_assert_eq!(PROJECT, "<project>", "canon_scope8 literal is out of step");
    text.replace(concat!("<project>", "/"), "")
}

/// The exact generator credit(s) sphinx-ultra renders in a page footer, one
/// per theme lineage/language that words it differently. Design decision 5:
/// pages must not claim "Created using Sphinx"; the oracle replaced Sphinx's
/// credit with [`CREDIT`], and every string listed here is replaced with the
/// same token on our side. T6 fills this with the wording it chooses (the
/// page comparison then shows exactly the footer line until it does).
const OUR_GENERATOR_CREDITS: &[&str] = &[];

fn normalize_credits(page: &str, credits: &[&str]) -> String {
    credits.iter().fold(page.to_string(), |text, credit| {
        text.replace(credit, CREDIT)
    })
}

/// Mask `?v=<8 hex>` after every local `_static/<path>` whose bytes differ
/// between the two builds (`differing`), leaving every other checksum
/// compared. A checksum is the CRC32 of the output file
/// (`sphinx/builders/html/_assets.py:111-135`), so for a byte-identical
/// file a checksum mismatch is a real divergence.
fn mask_checksums(page: &str, differing: &BTreeSet<String>) -> String {
    if differing.is_empty() {
        return page.to_string();
    }
    let pattern = checksum_pattern();
    pattern
        .replace_all(page, |caps: &regex::Captures<'_>| {
            let path = &caps[2];
            if differing.contains(path) {
                format!("{}{path}?v=<differs>", &caps[1])
            } else {
                caps[0].to_string()
            }
        })
        .into_owned()
}

fn checksum_pattern() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        regex::Regex::new(r#"((?:\.\./)*)(_static/[^"'?\s]+)\?v=[0-9a-f]{8}"#)
            .expect("checksum pattern compiles")
    })
}

// ---------------------------------------------------------------------------
// Running the library build over a fixture project
// ---------------------------------------------------------------------------

/// Paths only our side may write, never compared. Each entry must match at
/// least one file of at least one build (checked in
/// [`output_file_sets_match_oracle`]), so a stale entry fails.
const OUR_ONLY_PREFIXES: &[(&str, &str)] = &[(
    ".sphinx-ultra-cache/",
    "the crate's doctree/env cache: `SphinxBuilder::new` defaults it to \
     `<outdir>/.sphinx-ultra-cache`, the analogue of the `<outdir>/.doctrees` \
     a plain `sphinx-build src out` writes (the oracle keeps its doctree dir \
     outside the outdir, so neither appears in the recorded tree)",
)];

/// Oracle files whose PRESENCE and bytes are not compared yet. Checked the
/// other way round: if our file ever equals the oracle's byte for byte the
/// entry is stale. (The oracle records no sha for these -- the bytes embed
/// tmp paths as search terms -- so staleness is judged by the file being
/// present on our side with a non-empty `Search.setIndex(` payload.)
const M3_FILES: &[(&str, &str)] = &[(
    "searchindex.js",
    "the search index is M3 (design decision 8); search.html and the static \
     search JS are wave-5 work and ARE compared",
)];

/// Per (project, path) set-membership divergences, see the module docs for
/// the `"*"` rules. `path` may name a file only one side has.
const KNOWN_FILE_SET_GAPS: &[(&str, &str, &str)] = &[];

/// Per (project, page) byte divergences after normalization.
const KNOWN_PAGE_GAPS: &[(&str, &str, &str)] = &[];

/// Per (project, file) sha256 divergences of `_static/`, `_images/`,
/// `_downloads/` and html_extra_path files.
const KNOWN_ASSET_GAPS: &[(&str, &str, &str)] = &[];

/// Per (project, `_sources/...`) text divergences.
const KNOWN_SOURCE_GAPS: &[(&str, &str, &str)] = &[];

/// Per-project `.buildinfo` divergences. A project whose fixture says
/// `buildinfo_modelable: false` hashed a Python value the crate cannot
/// `str()`; such an entry should say so.
const KNOWN_BUILDINFO_GAPS: &[(&str, &str)] = &[];

/// Per-project objects.inv (header + decompressed payload) divergences.
const KNOWN_INVENTORY_GAPS: &[(&str, &str)] = &[];

/// Per-project warning-stream divergences. Sound only for MISSING warnings:
/// an exempted project must still emit a sub-multiset of the oracle's
/// records ([`assert_warning_gap_is_sound`]).
const KNOWN_WARNING_GAPS: &[(&str, &str)] = &[];

/// Fixture `conf` keys proven to steer nothing the crate implements, left
/// off the override pass. Empty: every key must apply as a `-D` override.
/// A key added here needs an arm in [`assert_inert_conf_is_sound`] (see the
/// same mechanism in tests/env_differential.rs).
const KNOWN_INERT_CONF: &[&str] = &[];

fn assert_inert_conf_is_sound(project: &str, key: &str, _value: &serde_json::Value) {
    panic!(
        "project {project:?} sets {key:?}, which is listed in KNOWN_INERT_CONF but has \
         no soundness arm in assert_inert_conf_is_sound -- add one (an empty arm saying \
         why EVERY value is inert, or an assertion narrowing the inert values)"
    );
}

/// One project's build: every output file (relpath with `/` separators ->
/// bytes, [`OUR_ONLY_PREFIXES`] files set aside), the warning stream as the
/// `-w` file would carry it, and the roots needed to normalize our text.
struct Built {
    files: BTreeMap<String, Vec<u8>>,
    our_only: Vec<String>,
    warnings: Vec<String>,
    source_root: String,
    output_root: String,
}

/// The project's `conf` as `-D key=value` pairs, in the fixture's (sorted)
/// key order; dict values expand to `key.sub` pairs.
fn overrides_of(project: &Project) -> Vec<(String, String)> {
    let conf = project
        .conf
        .as_object()
        .unwrap_or_else(|| panic!("project {}: conf is not an object", project.name));
    let mut overrides = Vec::new();
    for (key, value) in conf {
        if KNOWN_INERT_CONF.contains(&key.as_str()) {
            assert_inert_conf_is_sound(&project.name, key, value);
            continue;
        }
        match value {
            serde_json::Value::Object(map) => {
                for (sub, v) in map {
                    let key = format!("{key}.{sub}");
                    let value = scalar_override(&project.name, &key, v);
                    overrides.push((key, value));
                }
            }
            other => overrides.push((key.clone(), scalar_override(&project.name, key, other))),
        }
    }
    overrides
}

/// One conf value as the string a `-D` override carries: scalars verbatim,
/// comma-free string lists comma-joined (`apply_override` splits on
/// commas). Anything else panics rather than stringify into a differently
/// configured build (the tests/env_differential.rs rule).
fn scalar_override(project: &str, key: &str, value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => value.to_string(),
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| match item {
                serde_json::Value::String(text) if !text.contains(',') => text.as_str(),
                other => panic!(
                    "project {project:?}: conf key {key:?} has array element {other}, which \
                     the comma-joined -D form cannot carry"
                ),
            })
            .collect::<Vec<_>>()
            .join(","),
        other => panic!(
            "project {project:?}: conf key {key:?} has the non-scalar value {other}, which \
             -D cannot express -- move it to the project's conf_py"
        ),
    }
}

/// Write the project's srcdir: conf.py, documents, text and binary members.
fn materialize(project: &Project, source_dir: &Path) {
    let write = |relpath: &str, bytes: &[u8]| {
        let path = source_dir.join(relpath);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, bytes).unwrap();
    };
    write("conf.py", project.conf_py.as_bytes());
    for (docname, body) in &project.files {
        write(&format!("{docname}.rst"), body.as_bytes());
    }
    for (relpath, body) in &project.data_files {
        write(relpath, body.as_bytes());
    }
    for (relpath, encoded) in &project.binary_files {
        let bytes = base64_decode(encoded)
            .unwrap_or_else(|e| panic!("project {}: {relpath}: bad base64: {e}", project.name));
        write(relpath, &bytes);
    }
}

/// Every file under `dir`, relpath (with `/`) -> bytes.
fn read_tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let path = entry.path();
            if path.is_dir() {
                walk(&path, &rel, out);
            } else {
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out);
    out
}

fn build_project(project: &Project) -> Built {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("source");
    let output_dir = tmp.path().join("build");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    materialize(project, &source_dir);
    // Warning locations come from walking the source tree, which resolves
    // symlinks (`/var` -> `/private/var` on macOS): normalize both roots the
    // same way so the token substitution lands.
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();
    let output_dir = sphinx_ultra::utils::canonicalize_simplified(&output_dir).unwrap();

    // `sphinx-build -b <builder> -D ... source build`: conf.py, then -D.
    let mut config = BuildConfig::from_conf_py(source_dir.join("conf.py"))
        .unwrap_or_else(|e| panic!("project {}: conf.py does not load: {e:#}", project.name));
    let mut overrides = overrides_of(project);
    if project.builder != "html" {
        // Design decision 3: the builder kind is a `BuildConfig` field
        // named `builder`, set by `-b`.
        overrides.push(("builder".to_string(), project.builder.clone()));
    }
    for (key, value) in overrides {
        let ignored = config
            .apply_override(&key, &value)
            .unwrap_or_else(|e| panic!("project {}: -D {key}={value}: {e:#}", project.name));
        assert!(
            ignored.is_none(),
            "project {}: -D {key}={value} was ignored: {}",
            project.name,
            ignored.unwrap()
        );
    }
    // config-inited warnings come first, rendered the way the CLI writes
    // them to the -w file (src/main.rs).
    let mut warnings: Vec<String> = config
        .validate()
        .into_iter()
        .map(|message| format!("WARNING: {message}"))
        .collect();

    // SOURCE_DATE_EPOCH is process-global; every build here runs inside
    // the single `built_projects` initializer, one at a time.
    let saved_epoch = std::env::var_os("SOURCE_DATE_EPOCH");
    match project.env.get("SOURCE_DATE_EPOCH") {
        Some(epoch) => std::env::set_var("SOURCE_DATE_EPOCH", epoch),
        None => std::env::remove_var("SOURCE_DATE_EPOCH"),
    }
    let mut builder = SphinxBuilder::new(config, source_dir.clone(), output_dir.clone())
        .unwrap_or_else(|e| panic!("project {}: builder setup failed: {e:#}", project.name));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let stats = runtime.block_on(builder.build());
    match saved_epoch {
        Some(epoch) => std::env::set_var("SOURCE_DATE_EPOCH", epoch),
        None => std::env::remove_var("SOURCE_DATE_EPOCH"),
    }
    let stats = stats.unwrap_or_else(|e| panic!("project {}: build failed: {e:#}", project.name));

    let source_root = source_dir.to_string_lossy().into_owned();
    let output_root = output_dir.to_string_lossy().into_owned();
    warnings.extend(stats.warning_details.iter().map(|warning| warning.render()));
    let warnings = warnings
        .iter()
        .map(|warning| normalize_paths(warning, &source_root, &output_root))
        .collect();

    let mut files = read_tree(&output_dir);
    let our_only: Vec<String> = files
        .keys()
        .filter(|path| {
            OUR_ONLY_PREFIXES
                .iter()
                .any(|(prefix, _)| path.starts_with(prefix))
        })
        .cloned()
        .collect();
    for path in &our_only {
        files.remove(path);
    }
    Built {
        files,
        our_only,
        warnings,
        source_root,
        output_root,
    }
}

/// Every project's build, run once for the whole test binary.
fn built_projects() -> &'static BTreeMap<String, Built> {
    static BUILDS: OnceLock<BTreeMap<String, Built>> = OnceLock::new();
    BUILDS.get_or_init(|| {
        all_projects()
            .map(|(_, project)| (project.name.clone(), build_project(project)))
            .collect()
    })
}

fn built_of(project: &Project) -> &'static Built {
    built_projects()
        .get(&project.name)
        .unwrap_or_else(|| panic!("no build for project {}", project.name))
}

/// Our page text, normalized exactly like the oracle's was.
fn normalize_our_page(bytes: &[u8], built: &Built) -> String {
    let text = String::from_utf8_lossy(bytes);
    canon_scope8(&normalize_credits(
        &normalize_paths(&text, &built.source_root, &built.output_root),
        OUR_GENERATOR_CREDITS,
    ))
}

/// `_static` paths whose bytes differ between our build and the oracle
/// (including files we did not write): their `?v=` checksums are masked.
fn differing_statics(project: &Project, built: &Built) -> BTreeSet<String> {
    project
        .expect
        .output_files
        .iter()
        .filter(|(path, file)| file.kind == "static" && path.starts_with("_static/"))
        .filter(|(path, file)| {
            built
                .files
                .get(path.as_str())
                .map(|bytes| sha256_hex(bytes))
                != file.sha256
        })
        .map(|(path, _)| path.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Exemption ledger
// ---------------------------------------------------------------------------

fn covers(entry: &(&str, &str, &str), project: &str, path: &str) -> bool {
    (entry.0 == "*" || entry.0 == project) && (entry.1 == "*" || entry.1 == path)
}

/// Strict bookkeeping for one exemption table over one comparison key.
struct Ledger {
    table: &'static str,
    entries: Vec<(&'static str, &'static str, &'static str)>,
    visits: Vec<usize>,
    stale: Vec<String>,
}

impl Ledger {
    fn new(table: &'static str, entries: &[(&'static str, &'static str, &'static str)]) -> Self {
        Self {
            table,
            entries: entries.to_vec(),
            visits: vec![0; entries.len()],
            stale: Vec::new(),
        }
    }

    fn per_project(table: &'static str, entries: &[(&'static str, &'static str)]) -> Self {
        let entries: Vec<_> = entries.iter().map(|(p, why)| (*p, "*", *why)).collect();
        Self::new(table, &entries)
    }

    /// Record one compared pair. Returns the exemption's reason when the
    /// pair is covered (the caller then does not report it); a covered pair
    /// that no longer diverges is recorded as stale.
    fn record(&mut self, project: &str, path: &str, diverges: bool) -> Option<&'static str> {
        let covering: Vec<usize> = (0..self.entries.len())
            .filter(|&i| covers(&self.entries[i], project, path))
            .collect();
        assert!(
            covering.len() <= 1,
            "{}: [{project}] {path} is covered by {} entries ({:?}) -- keep one",
            self.table,
            covering.len(),
            covering
                .iter()
                .map(|&i| self.entries[i])
                .collect::<Vec<_>>()
        );
        let &index = covering.first()?;
        self.visits[index] += 1;
        let (p, q, why) = self.entries[index];
        if !diverges {
            self.stale.push(format!(
                "{} entry ({p}, {q}) -- {why} -- covers [{project}] {path}, which now \
                 MATCHES the oracle: narrow or delete the exemption",
                self.table
            ));
        }
        Some(why)
    }

    /// Stale entries: covered pairs that match, and entries covering nothing.
    fn finish(mut self) -> Vec<String> {
        for (index, (p, q, why)) in self.entries.iter().enumerate() {
            if self.visits[index] == 0 {
                self.stale.push(format!(
                    "{} entry ({p}, {q}) -- {why} -- covers no compared pair: no such \
                     project/path in the fixture. Delete it.",
                    self.table
                ));
            }
        }
        self.stale
    }
}

/// How many divergence entries a failing test lists before summarizing.
const REPORT_LIMIT: usize = 60;

fn report(key: &str, divergences: &[String], stale: &[String], extra: &str) {
    if divergences.is_empty() && stale.is_empty() {
        return;
    }
    let mut listed = divergences[..divergences.len().min(REPORT_LIMIT)].join("\n");
    if divergences.len() > REPORT_LIMIT {
        listed.push_str(&format!(
            "\n... and {} more",
            divergences.len() - REPORT_LIMIT
        ));
    }
    panic!(
        "{} divergence(s) vs the sphinx 9.1.0 HTML oracle ({key}), {} stale exemption(s):\n\n\
         {listed}\n\n{}{extra}",
        divergences.len(),
        stale.len(),
        stale.join("\n"),
    );
}

// ---------------------------------------------------------------------------
// Live now: fixture loading, pins, self-consistency, helpers, arithmetic
// ---------------------------------------------------------------------------

#[test]
fn fixtures_are_pinned_to_the_oracle_environment() {
    let files = fixture_files();
    assert_eq!(files.len(), FIXTURES.len());
    let pins: BTreeMap<String, String> = EXPECTED_PINS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    for ((family, _), file) in FIXTURES.iter().zip(files) {
        let at = format!("html_differential_{family}.json");
        assert_eq!(file.schema_version, SCHEMA_VERSION, "{at}: schema version");
        assert_eq!(
            file.generator, "tools/gen_html_fixture.py",
            "{at}: generator"
        );
        assert_eq!(file.family, *family, "{at}: family");
        assert_eq!(file.sphinx_version, "9.1.0", "{at}: sphinx");
        assert_eq!(file.docutils_version, "0.22.4", "{at}: docutils");
        assert_eq!(file.pins, pins, "{at}: pins");
        assert_eq!(file.pillow, "absent", "{at}: Pillow must be absent");
        assert_eq!(
            file.tokens,
            Tokens {
                project: PROJECT.to_string(),
                outdir: OUTDIR.to_string(),
                generator_credit: CREDIT.to_string(),
            },
            "{at}: normalization tokens"
        );
        assert_eq!(
            file.base_conf,
            serde_json::json!({
                "highlight_language": "none",
                "html_theme": "basic",
                "language": "en",
                "smartquotes": false,
            }),
            "{at}: base configuration"
        );
    }
}

/// The `.buildinfo` reference table is the same plain build in every file,
/// and holds exactly the 62 `rebuild='html'` options of a default Sphinx
/// 9.1 install (research/htmlbuilder.md §13.3).
#[test]
fn buildinfo_reference_is_the_62_html_options_everywhere() {
    let files = fixture_files();
    let first = &files[0].buildinfo_reference;
    assert_eq!(first.len(), 62, "html-rebuild option count");
    for name in [
        "copyright",
        "html_theme",
        "templates_path",
        "mathjax_path",
        "qthelp_basename",
    ] {
        assert!(
            first.contains_key(name),
            "{name} missing from the reference table"
        );
    }
    assert_eq!(first["html_theme"].repr, "'basic'");
    for file in &files[1..] {
        assert_eq!(
            &file.buildinfo_reference, first,
            "{}: a different buildinfo reference",
            file.family
        );
    }
    for (file, project) in all_projects() {
        for name in project.expect.buildinfo_config.keys() {
            assert!(
                file.buildinfo_reference.contains_key(name),
                "[{}] buildinfo delta names {name}, absent from the reference",
                project.name
            );
        }
        assert_eq!(
            project.expect.buildinfo_modelable,
            project.expect.buildinfo_unmodelable_leaves.is_empty(),
            "[{}] buildinfo_modelable disagrees with its leaf list",
            project.name
        );
    }
}

/// Every recorded text hashes to its recorded sha256, every page/source/
/// context entry is in a bijection with its output_files entry, and the
/// fixture carries no tmp path. Also proves [`sha256_hex`] against ~1000
/// real files.
#[test]
fn fixtures_are_self_consistent() {
    let mut names = BTreeSet::new();
    for (file, project) in all_projects() {
        let name = &project.name;
        assert!(names.insert(name.clone()), "project name {name} repeats");
        assert_eq!(project.family, file.family, "[{name}] family");
        assert!(
            ["html", "dirhtml"].contains(&project.builder.as_str()),
            "[{name}] builder {}",
            project.builder
        );
        assert!(project.files.contains_key("index"), "[{name}] has no index");
        assert!(
            project.conf_py.starts_with("project = 'fixture'\n"),
            "[{name}] conf.py does not start from the generator's CONF_PY"
        );
        for key in project.env.keys() {
            assert_eq!(key, "SOURCE_DATE_EPOCH", "[{name}] unexpected env var");
        }
        for key in &project.unset {
            assert!(
                file.base_conf.get(key).is_some() && project.conf.get(key).is_none(),
                "[{name}] unsets {key}, which is not a base key or is set anyway"
            );
        }
        // Must not panic: every conf value is -D-expressible.
        let _ = overrides_of(project);
        for relpath in project.data_files.keys() {
            assert!(
                !relpath.ends_with(".rst")
                    && !relpath.ends_with(".md")
                    && !relpath.ends_with(".txt"),
                "[{name}] data file {relpath} has a document suffix"
            );
        }
        for (relpath, encoded) in &project.binary_files {
            let bytes = base64_decode(encoded)
                .unwrap_or_else(|e| panic!("[{name}] {relpath}: bad base64: {e}"));
            if relpath.ends_with(".png") {
                assert!(
                    bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                    "[{name}] {relpath}: not a PNG"
                );
            }
        }

        let expect = &project.expect;
        for (path, entry) in &expect.output_files {
            assert!(
                OUTPUT_KINDS.contains(&entry.kind.as_str()),
                "[{name}] {path}: unknown kind {}",
                entry.kind
            );
            assert_eq!(
                entry.sha256.is_none(),
                entry.kind == "searchindex",
                "[{name}] {path}: only searchindex.js goes without a sha256"
            );
            let recorded: Option<String> = match entry.kind.as_str() {
                "page" => expect.pages.get(path).map(|lines| lines.join("\n")),
                "source" => expect.sources.get(path).cloned(),
                "buildinfo" => Some(expect.buildinfo.clone()),
                "static" if entry.text => {
                    let sha = entry.sha256.as_ref().unwrap();
                    let lines = file.texts.get(sha).unwrap_or_else(|| {
                        panic!("[{name}] {path}: text {sha} missing from the texts table")
                    });
                    Some(lines.join("\n"))
                }
                _ => None,
            };
            assert_eq!(
                entry.text,
                recorded.is_some(),
                "[{name}] {path}: the text flag disagrees with what is recorded"
            );
            if let Some(text) = recorded {
                assert_eq!(
                    Some(sha256_hex(text.as_bytes())),
                    entry.sha256,
                    "[{name}] {path}: recorded text does not hash to its sha256"
                );
            }
        }
        let of_kind = |kind: &str| -> BTreeSet<&String> {
            expect
                .output_files
                .iter()
                .filter(|(_, e)| e.kind == kind)
                .map(|(p, _)| p)
                .collect()
        };
        assert_eq!(
            of_kind("page"),
            expect.pages.keys().collect(),
            "[{name}] pages vs output_files"
        );
        assert_eq!(
            of_kind("page"),
            expect.page_context.keys().collect(),
            "[{name}] page_context vs output_files"
        );
        assert_eq!(
            of_kind("source"),
            expect.sources.keys().collect(),
            "[{name}] sources vs output_files"
        );
        for kind in ["buildinfo", "inventory", "searchindex"] {
            assert_eq!(of_kind(kind).len(), 1, "[{name}] exactly one {kind}");
        }
        assert!(expect.output_files.contains_key(".buildinfo"));
        assert!(expect.output_files.contains_key("objects.inv"));
        assert!(expect.output_files.contains_key("searchindex.js"));

        let lines: Vec<&str> = expect.buildinfo.split('\n').collect();
        assert_eq!(
            lines.len(),
            5,
            "[{name}] .buildinfo is 4 lines + final newline"
        );
        assert_eq!(lines[0], "# Sphinx build info version 1");
        assert!(lines[2].starts_with("config: ") && lines[2].len() == 8 + 32);
        let tags = if project.builder == "html" {
            HTML_TAGS_HASH
        } else {
            DIRHTML_TAGS_HASH
        };
        assert_eq!(lines[3], format!("tags: {tags}"), "[{name}] tags hash");
        assert_eq!(lines[4], "");

        let header = &expect.inventory.header;
        assert_eq!(header.len(), 4, "[{name}] inventory header");
        assert_eq!(header[0], "# Sphinx inventory version 2");
        assert!(header[1].starts_with("# Project: "));
        assert!(header[2].starts_with("# Version: "));
        assert_eq!(
            header[3],
            "# The remainder of this file is compressed using zlib."
        );

        // Credits: one per footer-bearing page iff show_sphinx.
        for (path, lines) in &expect.pages {
            let context = &expect.page_context[path];
            let show_sphinx =
                context.ctx.get("show_sphinx") == Some(&serde_json::Value::Bool(true));
            let has_footer = context.template != "opensearch.xml"
                && !CREDITLESS_THEMES.contains(&theme_of(project));
            let found = lines
                .iter()
                .map(|l| l.matches(CREDIT).count())
                .sum::<usize>();
            assert_eq!(
                found,
                usize::from(show_sphinx && has_footer),
                "[{name}] {path}: generator credit tokens"
            );
            assert!(!context.pagename.is_empty());
        }

        // No tmp path survived anywhere.
        let mut texts: Vec<String> = expect.pages.values().map(|l| l.join("\n")).collect();
        texts.extend(expect.warnings.iter().cloned());
        texts.extend(expect.sources.values().cloned());
        texts.push(
            serde_json::to_string(
                &expect
                    .page_context
                    .values()
                    .map(|c| &c.addctx)
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        );
        texts.push(
            serde_json::to_string(
                &expect
                    .page_context
                    .values()
                    .map(|c| &c.ctx)
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        );
        for text in &texts {
            assert!(
                !text.contains("html_oracle_") && !text.contains("/tmp/"),
                "[{name}] a tmp path leaked into the fixture"
            );
        }
    }
}

#[test]
fn sha256_matches_known_answers() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

#[test]
fn base64_matches_known_answers() {
    assert_eq!(base64_decode("").unwrap(), b"");
    assert_eq!(base64_decode("Zg==").unwrap(), b"f");
    assert_eq!(base64_decode("Zm8=").unwrap(), b"fo");
    assert_eq!(base64_decode("Zm9vYmFy").unwrap(), b"foobar");
    assert_eq!(base64_decode("/+8=").unwrap(), [0xff, 0xef]);
    assert!(base64_decode("Zg=").is_err());
    assert!(base64_decode("Zg==Zg==").is_err());
    assert!(base64_decode("Z!==").is_err());
}

#[test]
fn unified_diff_is_compact() {
    assert_eq!(unified_diff("a\nb", "a\nb", 3, 50), "");
    let old = (1..=20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let new = old.replace("line 10", "line ten").replace("line 18\n", "");
    assert_eq!(
        unified_diff(&old, &new, 1, 50),
        "--- oracle\n+++ ours\n\
         @@ -9,3 +9,3 @@\n line 9\n-line 10\n+line ten\n line 11\n\
         @@ -17,3 +17,2 @@\n line 17\n-line 18\n line 19"
    );
    // Hunks closer than 2*context merge; truncation says how much is hidden.
    let merged = unified_diff(&old, &new.replace("line 12", "line twelve"), 1, 6);
    assert!(
        merged.starts_with("--- oracle\n+++ ours\n@@ -9,5 +9,5 @@\n line 9\n-line 10\n+line ten")
    );
    assert!(merged.ends_with("more diff lines)"));
    assert_eq!(first_differing_line(&old, &new), 10);
    assert_eq!(first_differing_line("a", "a\nb"), 2);
}

#[test]
fn page_regions_follow_the_basic_layout() {
    let page = "<!DOCTYPE html>\n<head>\n</head><body>\n<div class=\"related\" role=\"navigation\">\n\
                <div class=\"document\">\n<div class=\"body\" role=\"main\">\n<p>x</p>\n\
                <div class=\"sphinxsidebar\" role=\"navigation\">\n<div class=\"footer\" role=\"contentinfo\">";
    assert_eq!(page_region(page, 1), "preamble");
    assert_eq!(page_region(page, 2), "head");
    assert_eq!(page_region(page, 4), "relbar");
    assert_eq!(page_region(page, 7), "body");
    assert_eq!(page_region(page, 8), "sidebar");
    assert_eq!(page_region(page, 9), "footer");
}

#[test]
fn normalizations_are_precise() {
    // Separators flip only inside placeholder-rooted paths.
    assert_eq!(
        normalize_paths(
            r"C:\t\source\sub\a.rst:3: WARNING: x\y in C:\t\build\_static\f.css",
            r"C:\t\source",
            r"C:\t\build",
        ),
        r"<project>/sub/a.rst:3: WARNING: x\y in <outdir>/_static/f.css"
    );
    assert_eq!(
        normalize_paths("/t/source/a.rst and /t/sourcery", "/t/source", "/t/build"),
        "<project>/a.rst and <project>ry"
    );
    assert_eq!(
        canon_scope8("<project>/a.rst:1: <project>"),
        "a.rst:1: <project>"
    );
    assert_eq!(
        normalize_credits("x Made by us 1.0. y", &["Made by us 1.0."]),
        format!("x {CREDIT} y")
    );
    let differing: BTreeSet<String> = ["_static/basic.css".to_string()].into();
    assert_eq!(
        mask_checksums(
            "href=\"../_static/basic.css?v=29da98fa\" src=\"_static/doctools.js?v=fd6eb6e6\"",
            &differing
        ),
        "href=\"../_static/basic.css?v=<differs>\" src=\"_static/doctools.js?v=fd6eb6e6\""
    );
}

/// The corpus counts this harness quotes, and the exemption arithmetic,
/// computed from the tables and the fixture. Each table is first proved
/// well-formed against the fixture (every entry covers at least one
/// fixture pair, no pair is covered twice), so the covered counts ARE the
/// exemption counts. Update the constants together with the report that
/// quotes them.
const DOCUMENTED_FIXTURE_FILES: usize = 10;
const DOCUMENTED_PROJECTS: usize = 133;
const DOCUMENTED_DIRHTML_PROJECTS: usize = 5;
const DOCUMENTED_OUTPUT_FILES: usize = 2957;
const DOCUMENTED_PAGES: usize = 590;
const DOCUMENTED_ASSETS: usize = 1691;
const DOCUMENTED_SOURCES: usize = 277;
const DOCUMENTED_WARNING_RECORDS: usize = 66;
const DOCUMENTED_EXEMPT_PAGES: usize = 0;
const DOCUMENTED_EXEMPT_ASSETS: usize = 0;
const DOCUMENTED_EXEMPT_SOURCES: usize = 0;
const DOCUMENTED_EXEMPT_BUILDINFO_PROJECTS: usize = 0;
const DOCUMENTED_EXEMPT_INVENTORY_PROJECTS: usize = 0;
const DOCUMENTED_EXEMPT_WARNING_PROJECTS: usize = 0;

/// How many fixture pairs a table covers, asserting it is well-formed.
fn covered_pairs(table: &str, entries: &[(&str, &str, &str)], pairs: &[(&str, &str)]) -> usize {
    let mut covered = 0;
    for &(project, path) in pairs {
        let count = entries.iter().filter(|e| covers(e, project, path)).count();
        assert!(count <= 1, "{table}: [{project}] {path} is covered twice");
        covered += count;
    }
    for entry in entries {
        assert!(
            pairs.iter().any(|&(p, q)| covers(entry, p, q)),
            "{table} entry {entry:?} covers no fixture pair"
        );
    }
    covered
}

#[test]
fn exemption_arithmetic_matches_the_documented_numbers() {
    let projects: Vec<&Project> = all_projects().map(|(_, p)| p).collect();
    let pairs_of = |kinds: &[&str]| -> Vec<(&str, &str)> {
        projects
            .iter()
            .flat_map(|p| {
                p.expect
                    .output_files
                    .iter()
                    .filter(|(_, e)| kinds.contains(&e.kind.as_str()))
                    .map(|(path, _)| (p.name.as_str(), path.as_str()))
            })
            .collect()
    };
    let per_project: Vec<(&str, &str)> = projects.iter().map(|p| (p.name.as_str(), "*")).collect();
    let as_triples = |entries: &[(&'static str, &'static str)]| -> Vec<(&str, &str, &str)> {
        entries.iter().map(|(p, why)| (*p, "*", *why)).collect()
    };

    let pages = pairs_of(&["page"]);
    let assets = pairs_of(ASSET_KINDS);
    let sources = pairs_of(&["source"]);
    let all_files: usize = projects.iter().map(|p| p.expect.output_files.len()).sum();
    let warning_records: usize = projects.iter().map(|p| p.expect.warnings.len()).sum();
    let dirhtml = projects.iter().filter(|p| p.builder == "dirhtml").count();

    let exempt_pages = covered_pairs("KNOWN_PAGE_GAPS", KNOWN_PAGE_GAPS, &pages);
    let exempt_assets = covered_pairs("KNOWN_ASSET_GAPS", KNOWN_ASSET_GAPS, &assets);
    let exempt_sources = covered_pairs("KNOWN_SOURCE_GAPS", KNOWN_SOURCE_GAPS, &sources);
    let exempt_buildinfo = covered_pairs(
        "KNOWN_BUILDINFO_GAPS",
        &as_triples(KNOWN_BUILDINFO_GAPS),
        &per_project,
    );
    let exempt_inventory = covered_pairs(
        "KNOWN_INVENTORY_GAPS",
        &as_triples(KNOWN_INVENTORY_GAPS),
        &per_project,
    );
    let exempt_warnings = covered_pairs(
        "KNOWN_WARNING_GAPS",
        &as_triples(KNOWN_WARNING_GAPS),
        &per_project,
    );
    // File-set entries may name files only our side writes, so they are
    // checked against the project list only.
    for (project, path, _) in KNOWN_FILE_SET_GAPS {
        assert!(
            *project == "*" || projects.iter().any(|p| p.name == *project),
            "KNOWN_FILE_SET_GAPS names unknown project {project} ({path})"
        );
    }

    let actual = [
        ("fixture files", FIXTURES.len()),
        ("projects", projects.len()),
        ("dirhtml projects", dirhtml),
        ("output files", all_files),
        ("pages", pages.len()),
        ("assets", assets.len()),
        ("sources", sources.len()),
        ("warning records", warning_records),
        ("exempt pages", exempt_pages),
        ("exempt assets", exempt_assets),
        ("exempt sources", exempt_sources),
        ("exempt buildinfo projects", exempt_buildinfo),
        ("exempt inventory projects", exempt_inventory),
        ("exempt warning projects", exempt_warnings),
    ];
    let documented = [
        ("fixture files", DOCUMENTED_FIXTURE_FILES),
        ("projects", DOCUMENTED_PROJECTS),
        ("dirhtml projects", DOCUMENTED_DIRHTML_PROJECTS),
        ("output files", DOCUMENTED_OUTPUT_FILES),
        ("pages", DOCUMENTED_PAGES),
        ("assets", DOCUMENTED_ASSETS),
        ("sources", DOCUMENTED_SOURCES),
        ("warning records", DOCUMENTED_WARNING_RECORDS),
        ("exempt pages", DOCUMENTED_EXEMPT_PAGES),
        ("exempt assets", DOCUMENTED_EXEMPT_ASSETS),
        ("exempt sources", DOCUMENTED_EXEMPT_SOURCES),
        (
            "exempt buildinfo projects",
            DOCUMENTED_EXEMPT_BUILDINFO_PROJECTS,
        ),
        (
            "exempt inventory projects",
            DOCUMENTED_EXEMPT_INVENTORY_PROJECTS,
        ),
        (
            "exempt warning projects",
            DOCUMENTED_EXEMPT_WARNING_PROJECTS,
        ),
    ];
    assert_eq!(
        actual,
        documented,
        "the corpus or a table changed: {} pages ({} byte-exact), {} assets ({} byte-exact), \
         {} sources, {} projects ({} buildinfo-exact, {} inventory-exact, {} \
         warning-exact) -- update the DOCUMENTED_* constants and the report quoting them",
        pages.len(),
        pages.len() - exempt_pages,
        assets.len(),
        assets.len() - exempt_assets,
        sources.len(),
        projects.len(),
        projects.len() - exempt_buildinfo,
        projects.len() - exempt_inventory,
        projects.len() - exempt_warnings,
    );
}

// ---------------------------------------------------------------------------
// The comparisons (M2 wave 5 T6)
// ---------------------------------------------------------------------------

/// The set of files each build writes, against the oracle's tree.
#[test]
#[ignore = "M2 wave 5 T6: HTML builder not wired yet"]
fn output_file_sets_match_oracle() {
    let mut ledger = Ledger::new("KNOWN_FILE_SET_GAPS", KNOWN_FILE_SET_GAPS);
    let mut divergences = Vec::new();
    let mut prefix_uses = vec![0usize; OUR_ONLY_PREFIXES.len()];
    let mut stale = Vec::new();

    for (_, project) in all_projects() {
        let built = built_of(project);
        for path in &built.our_only {
            for (index, (prefix, _)) in OUR_ONLY_PREFIXES.iter().enumerate() {
                if path.starts_with(prefix) {
                    prefix_uses[index] += 1;
                }
            }
        }
        let is_m3 = |path: &str| M3_FILES.iter().any(|(p, _)| *p == path);
        let ours: BTreeSet<&str> = built
            .files
            .keys()
            .map(String::as_str)
            .filter(|p| !is_m3(p))
            .collect();
        let oracle: BTreeSet<&str> = project
            .expect
            .output_files
            .keys()
            .map(String::as_str)
            .filter(|p| !is_m3(p))
            .collect();
        for (m3, why) in M3_FILES {
            if let Some(bytes) = built.files.get(*m3) {
                // No oracle bytes to compare with (see M3_FILES); a
                // complete-looking index means the entry needs revisiting.
                let text = String::from_utf8_lossy(bytes);
                if text.contains("\"terms\":{\"") && text.contains("\"titleterms\":{\"") {
                    stale.push(format!(
                        "[{}] {m3} carries a populated search index -- M3_FILES ({why}) \
                         may be stale: record its bytes in the oracle and compare them",
                        project.name
                    ));
                }
            }
        }
        for path in ours.union(&oracle) {
            let side = match (ours.contains(path), oracle.contains(path)) {
                (true, true) => None,
                (false, true) => Some("missing on our side"),
                _ => Some("written only on our side"),
            };
            if ledger.record(&project.name, path, side.is_some()).is_some() {
                continue;
            }
            if let Some(side) = side {
                divergences.push(format!("[{}] {path}: {side}", project.name));
            }
        }
    }
    for (index, (prefix, why)) in OUR_ONLY_PREFIXES.iter().enumerate() {
        if prefix_uses[index] == 0 {
            stale.push(format!(
                "OUR_ONLY_PREFIXES entry {prefix} ({why}) matched no file of any build -- \
                 delete it"
            ));
        }
    }
    stale.extend(ledger.finish());
    report("output file set", &divergences, &stale, "");
}

/// Every page, byte for byte after the module-level normalizations.
#[test]
#[ignore = "M2 wave 5 T6: HTML builder not wired yet"]
fn pages_match_oracle() {
    let mut ledger = Ledger::new("KNOWN_PAGE_GAPS", KNOWN_PAGE_GAPS);
    let mut divergences = Vec::new();
    let mut first_diff: Option<String> = None;

    for (_, project) in all_projects() {
        let built = built_of(project);
        let differing = differing_statics(project, built);
        for (path, lines) in &project.expect.pages {
            let expected = mask_checksums(&canon_scope8(&lines.join("\n")), &differing);
            let actual = built
                .files
                .get(path)
                .map(|bytes| mask_checksums(&normalize_our_page(bytes, built), &differing));
            let matches = actual.as_deref() == Some(expected.as_str());
            if ledger.record(&project.name, path, !matches).is_some() || matches {
                continue;
            }
            let Some(actual) = actual else {
                divergences.push(format!("[{}] {path}: not written", project.name));
                continue;
            };
            let line = first_differing_line(&expected, &actual);
            let context = &project.expect.page_context[path];
            let body = context
                .addctx
                .get("body")
                .and_then(|b| b.as_str())
                .map(canon_scope8);
            let body_note = match body {
                Some(body) if actual.contains(&body) => "; the oracle body is intact in ours",
                Some(_) => "; the oracle body is NOT in ours verbatim",
                None => "",
            };
            divergences.push(format!(
                "[{}] {path} ({}): first difference at line {line}, in the {}{body_note}",
                project.name,
                context.template,
                page_region(&expected, line)
            ));
            if first_diff.is_none() {
                first_diff = Some(format!(
                    "\n\nunified diff of the first divergent page, [{}] {path}:\n{}",
                    project.name,
                    unified_diff(&expected, &actual, 3, 120)
                ));
            }
        }
    }
    let stale = ledger.finish();
    report(
        "pages",
        &divergences,
        &stale,
        &first_diff.unwrap_or_default(),
    );
}

/// `_static/`, `_images/`, `_downloads/` and html_extra_path files by
/// sha256; templated statics (basic.css, documentation_options.js, ...)
/// print a diff against the recorded text.
#[test]
#[ignore = "M2 wave 5 T6: HTML builder not wired yet"]
fn assets_match_oracle() {
    let mut ledger = Ledger::new("KNOWN_ASSET_GAPS", KNOWN_ASSET_GAPS);
    let mut divergences = Vec::new();
    let mut first_diff: Option<String> = None;

    for (file, project) in all_projects() {
        let built = built_of(project);
        for (path, entry) in &project.expect.output_files {
            if !ASSET_KINDS.contains(&entry.kind.as_str()) {
                continue;
            }
            let ours = built.files.get(path);
            let matches = ours.map(|bytes| sha256_hex(bytes)) == entry.sha256;
            if ledger.record(&project.name, path, !matches).is_some() || matches {
                continue;
            }
            divergences.push(format!(
                "[{}] {path} ({}): {}",
                project.name,
                entry.kind,
                if ours.is_some() {
                    "bytes differ"
                } else {
                    "not written"
                }
            ));
            if let (None, Some(ours), true) = (&first_diff, ours, entry.text) {
                let oracle = file.texts[entry.sha256.as_ref().unwrap()].join("\n");
                first_diff = Some(format!(
                    "\n\nunified diff of the first divergent templated static, [{}] {path}:\n{}",
                    project.name,
                    unified_diff(&oracle, &String::from_utf8_lossy(ours), 3, 80)
                ));
            }
        }
    }
    let stale = ledger.finish();
    report(
        "assets",
        &divergences,
        &stale,
        &first_diff.unwrap_or_default(),
    );
}

/// `_sources/*`: byte copies of the inputs under Sphinx's name rule
/// (`<docname><source_suffix>[<html_sourcelink_suffix>]`).
#[test]
#[ignore = "M2 wave 5 T6: HTML builder not wired yet"]
fn sources_match_oracle() {
    let mut ledger = Ledger::new("KNOWN_SOURCE_GAPS", KNOWN_SOURCE_GAPS);
    let mut divergences = Vec::new();
    for (_, project) in all_projects() {
        let built = built_of(project);
        for (path, expected) in &project.expect.sources {
            let ours = built.files.get(path);
            let matches = ours.map(Vec::as_slice) == Some(expected.as_bytes());
            if ledger.record(&project.name, path, !matches).is_some() || matches {
                continue;
            }
            divergences.push(match ours {
                None => format!("[{}] {path}: not written", project.name),
                Some(ours) => format!(
                    "[{}] {path}:\n{}",
                    project.name,
                    unified_diff(expected, &String::from_utf8_lossy(ours), 2, 30)
                ),
            });
        }
    }
    let stale = ledger.finish();
    report("_sources", &divergences, &stale, "");
}

/// `.buildinfo`, verbatim: the config hash is `stable_hash` over every
/// `rebuild='html'` option (research/htmlbuilder.md §13); a mismatch
/// prints the project's recorded per-option delta to localize it.
#[test]
#[ignore = "M2 wave 5 T6: HTML builder not wired yet"]
fn buildinfo_matches_oracle() {
    let mut ledger = Ledger::per_project("KNOWN_BUILDINFO_GAPS", KNOWN_BUILDINFO_GAPS);
    let mut divergences = Vec::new();
    for (_, project) in all_projects() {
        let built = built_of(project);
        let ours = built.files.get(".buildinfo");
        let matches = ours.map(Vec::as_slice) == Some(project.expect.buildinfo.as_bytes());
        if ledger
            .record(&project.name, ".buildinfo", !matches)
            .is_some()
            || matches
        {
            continue;
        }
        let delta: Vec<String> = project
            .expect
            .buildinfo_config
            .iter()
            .map(|(name, value)| format!("    {name} = {} (hash {})", value.repr, value.hash))
            .collect();
        divergences.push(format!(
            "[{}] .buildinfo{}\n  oracle: {:?}\n  ours:   {:?}\n  options differing from \
             the reference build:\n{}",
            project.name,
            if project.expect.buildinfo_modelable {
                ""
            } else {
                " (the oracle hashed a value the crate cannot str(): see buildinfo_unmodelable_leaves)"
            },
            project.expect.buildinfo,
            ours.map(|b| String::from_utf8_lossy(b).into_owned()),
            if delta.is_empty() {
                "    (none)".to_string()
            } else {
                delta.join("\n")
            }
        ));
    }
    let stale = ledger.finish();
    report(".buildinfo", &divergences, &stale, "");
}

/// objects.inv: the 4 header lines and the zlib-DECOMPRESSED payload
/// (compressed bytes differ across zlib implementations; decision 8).
#[test]
#[ignore = "M2 wave 5 T6: HTML builder not wired yet"]
fn inventories_match_oracle() {
    fn split(bytes: &[u8]) -> Option<(Vec<String>, String)> {
        let mut rest = bytes;
        let mut header = Vec::new();
        for _ in 0..4 {
            let newline = rest.iter().position(|b| *b == b'\n')?;
            header.push(String::from_utf8(rest[..newline].to_vec()).ok()?);
            rest = &rest[newline + 1..];
        }
        let mut payload = String::new();
        flate2::read::ZlibDecoder::new(rest)
            .read_to_string(&mut payload)
            .ok()?;
        Some((header, payload))
    }

    let mut ledger = Ledger::per_project("KNOWN_INVENTORY_GAPS", KNOWN_INVENTORY_GAPS);
    let mut divergences = Vec::new();
    for (_, project) in all_projects() {
        let built = built_of(project);
        let expected = &project.expect.inventory;
        let ours = built.files.get("objects.inv").and_then(|b| split(b));
        let matches = ours.as_ref().is_some_and(|(header, payload)| {
            header == &expected.header && payload == &expected.payload
        });
        if ledger
            .record(&project.name, "objects.inv", !matches)
            .is_some()
            || matches
        {
            continue;
        }
        divergences.push(match ours {
            None => format!(
                "[{}] objects.inv: missing or not a v2 inventory",
                project.name
            ),
            Some((header, payload)) => format!(
                "[{}] objects.inv\n  header oracle: {:?}\n  header ours:   {header:?}\n{}",
                project.name,
                expected.header,
                unified_diff(&expected.payload, &payload, 1, 40)
            ),
        });
    }
    let stale = ledger.finish();
    report("objects.inv", &divergences, &stale, "");
}

/// The side-condition that makes a [`KNOWN_WARNING_GAPS`] entry sound (the
/// tests/env_differential.rs rule): the exemption covers MISSING warnings
/// only, so ours must be a sub-multiset of the oracle's records.
fn assert_warning_gap_is_sound(project: &str, actual: &[String], expected: &[String]) {
    let mut budget: BTreeMap<&String, usize> = BTreeMap::new();
    for record in expected {
        *budget.entry(record).or_default() += 1;
    }
    let extra: Vec<&String> = actual
        .iter()
        .filter(|record| match budget.get_mut(record) {
            Some(remaining) if *remaining > 0 => {
                *remaining -= 1;
                false
            }
            _ => true,
        })
        .collect();
    assert!(
        extra.is_empty(),
        "[{project}] is in KNOWN_WARNING_GAPS but emits records the oracle does not (or \
         more often): {extra:#?}"
    );
}

/// The warning stream, one record per warning (a multi-line reporter record
/// is one record), config-inited warnings first as the -w file has them.
#[test]
#[ignore = "M2 wave 5 T6: HTML builder not wired yet"]
fn warnings_match_oracle() {
    let mut ledger = Ledger::per_project("KNOWN_WARNING_GAPS", KNOWN_WARNING_GAPS);
    let mut divergences = Vec::new();
    for (_, project) in all_projects() {
        let actual: Vec<String> = built_of(project)
            .warnings
            .iter()
            .map(|w| canon_scope8(w))
            .collect();
        let expected: Vec<String> = project
            .expect
            .warnings
            .iter()
            .map(|w| canon_scope8(w))
            .collect();
        let matches = actual == expected;
        if ledger.record(&project.name, "warnings", !matches).is_some() {
            assert_warning_gap_is_sound(&project.name, &actual, &expected);
            continue;
        }
        if !matches {
            divergences.push(format!(
                "[{}] warnings\n  expected: {expected:#?}\n  actual:   {actual:#?}",
                project.name
            ));
        }
    }
    let stale = ledger.finish();
    report("warnings", &divergences, &stale, "");
}
