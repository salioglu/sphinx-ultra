//! Round-trip tests for `objects.inv` against the real Sphinx INVENTORY
//! ORACLE (`tests/fixtures/inventories/`): reader correctness against every
//! committed `.inv`'s expected parse table, writer round-trip through our
//! own reader, byte-exact decompressed-payload lines against the oracle's
//! own lines (fed the exact pre-compaction records Sphinx's own writer
//! used), the exact `ValueError` text for every malformed-header case, and
//! the binary-safety property (a compressed tail containing bare CR/LF
//! bytes and invalid UTF-8) that the old lossy-UTF8-then-`.lines()` reader
//! used to corrupt.
//!
//! Regenerate the fixture (manual, never in CI):
//!     uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' \
//!         python tools/gen_inventory_fixture.py

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::ZlibDecoder;
use sphinx_ultra::{InvObject, Inventory, InventoryFile};

#[derive(serde::Deserialize)]
struct Manifest {
    #[allow(dead_code)]
    sphinx_version: String,
    #[allow(dead_code)]
    docutils_version: String,
    #[allow(dead_code)]
    generator: String,
    uri_base: String,
    unsafe_utf8_case: String,
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    kind: String,
    #[serde(default)]
    #[allow(dead_code)]
    source: Option<String>,
    #[allow(dead_code)]
    covers: String,
    inv_file: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    expect: BTreeMap<String, BTreeMap<String, ExpectedItem>>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    raw_objects: Option<BTreeMap<String, Vec<RawObject>>>,
}

#[derive(serde::Deserialize, Debug, Clone, PartialEq, Eq)]
struct ExpectedItem {
    project_name: String,
    project_version: String,
    uri: String,
    display_name: String,
}

/// One `domain.get_objects()` record exactly as Sphinx's own writer
/// consumed it, pre-`$`-compaction (see `tools/gen_inventory_fixture.py`'s
/// `capture_raw_objects`).
#[derive(serde::Deserialize, Clone)]
struct RawObject {
    name: String,
    dispname: String,
    objtype: String,
    docname: String,
    anchor: String,
    priority: i32,
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/inventories")
}

fn load_manifest() -> Manifest {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/inventories/manifest.json"
    ));
    serde_json::from_str(raw).expect("tests/fixtures/inventories/manifest.json parses")
}

fn load_inv_bytes(inv_file: &str) -> Vec<u8> {
    std::fs::read(fixtures_dir().join(inv_file))
        .unwrap_or_else(|e| panic!("failed to read fixture {inv_file}: {e}"))
}

/// `Inventory` -> the same `{objtype: {name: {...}}}` shape the manifest's
/// `expect` field uses, for direct comparison.
fn inventory_to_table(inv: &Inventory) -> BTreeMap<String, BTreeMap<String, ExpectedItem>> {
    inv.data
        .iter()
        .map(|(objtype, names)| {
            let names = names
                .iter()
                .map(|(name, item)| {
                    (
                        name.clone(),
                        ExpectedItem {
                            project_name: item.project_name.clone(),
                            project_version: item.project_version.clone(),
                            uri: item.uri.clone(),
                            display_name: item.display_name.clone(),
                        },
                    )
                })
                .collect();
            (objtype.clone(), names)
        })
        .collect()
}

/// Byte offset just past the 4th `\n` in a v2 `.inv` file (i.e. the start of
/// the raw zlib tail), found the same way `_loads_v2` frames it:
/// `split(b'\n', maxsplit=3)`'s 4th part.
fn compressed_tail(raw: &[u8]) -> &[u8] {
    let mut newline_positions = Vec::with_capacity(4);
    for (i, &b) in raw.iter().enumerate() {
        if b == b'\n' {
            newline_positions.push(i);
            if newline_positions.len() == 4 {
                break;
            }
        }
    }
    assert_eq!(
        newline_positions.len(),
        4,
        "expected a well-formed v2 header (4 newlines) in this fixture"
    );
    &raw[newline_positions[3] + 1..]
}

fn decompressed_lines_from_inv_bytes(raw: &[u8]) -> Vec<String> {
    let compressed = compressed_tail(raw);
    let mut decoder = ZlibDecoder::new(compressed);
    let mut decompressed = String::new();
    decoder
        .read_to_string(&mut decompressed)
        .expect("decompress inventory payload");
    decompressed.lines().map(|l| l.to_string()).collect()
}

// ---------------------------------------------------------------------------
// Test 1: rust-parse committed .inv == expected table (per case)
// ---------------------------------------------------------------------------

#[test]
fn ok_cases_parse_to_expected_table() {
    let manifest = load_manifest();
    let mut failures = Vec::new();

    for case in manifest.cases.iter().filter(|c| c.kind == "ok") {
        let bytes = load_inv_bytes(&case.inv_file);
        match InventoryFile::loads(&bytes, &manifest.uri_base) {
            Ok(inv) => {
                let actual = inventory_to_table(&inv);
                if actual != case.expect {
                    failures.push(format!(
                        "case {:?}: parsed table differs from expected.\n  expected: {:#?}\n  actual:   {:#?}",
                        case.name, case.expect, actual
                    ));
                }
            }
            Err(e) => failures.push(format!("case {:?}: expected Ok, got Err({e})", case.name)),
        }
    }

    assert!(
        failures.is_empty(),
        "{} ok-case mismatch(es):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

// ---------------------------------------------------------------------------
// Test 2: malformed headers raise the exact ValueError text
// (docs/superpowers/plans/2026-08-31-m2-wave4-research-spec-inventory-intersphinx.md §1)
// ---------------------------------------------------------------------------

#[test]
fn error_cases_produce_exact_message() {
    let manifest = load_manifest();
    let mut failures = Vec::new();

    for case in manifest.cases.iter().filter(|c| c.kind == "error") {
        let bytes = load_inv_bytes(&case.inv_file);
        let expected = case
            .error
            .as_deref()
            .unwrap_or_else(|| panic!("error case {:?} has no `error` field", case.name));
        match InventoryFile::loads(&bytes, &manifest.uri_base) {
            Ok(_) => failures.push(format!(
                "case {:?}: expected Err({expected:?}), got Ok",
                case.name
            )),
            Err(e) => {
                let actual = e.to_string();
                if actual != expected {
                    failures.push(format!(
                        "case {:?}: error text mismatch.\n  expected: {expected:?}\n  actual:   {actual:?}",
                        case.name
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} error-case mismatch(es):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

// ---------------------------------------------------------------------------
// Test 3: writer round trip -- InvObjects reconstructed from an expected
// table, dumped, re-parsed with our own reader, must reproduce the exact
// same table. Covers every "ok" case, including the v1-sourced and
// absolute-location ones: the writer always emits v2 regardless of how the
// source table was originally produced, and posix_join's absolute-location
// override is exercised end to end by the v2_absolute_location case (its
// reconstructed "docname" is itself an absolute path).
// ---------------------------------------------------------------------------

/// Decompose one case's `expect` table back into `InvObject`s a writer could
/// plausibly have started from. `priority` is arbitrary (0) since `loads`
/// never records it (`_InventoryItem` has no such field) -- round-trip
/// identity doesn't depend on it. `dispname` is reconstructed as `name`
/// when `display_name == "-"` (the compaction our writer will reapply) and
/// as `display_name` verbatim otherwise, so a second `loads` of our dump
/// reproduces exactly the original table.
fn invobjects_from_expected_table(
    table: &BTreeMap<String, BTreeMap<String, ExpectedItem>>,
    uri_base: &str,
) -> BTreeMap<String, Vec<InvObject>> {
    let mut by_domain: BTreeMap<String, Vec<InvObject>> = BTreeMap::new();

    for (type_key, names) in table {
        let (domain, objtype) = type_key
            .split_once(':')
            .unwrap_or_else(|| panic!("objtype key {type_key:?} has no domain prefix"));

        for (name, item) in names {
            let remainder = item.uri.strip_prefix(uri_base).unwrap_or(item.uri.as_str());
            let (docname, anchor) = match remainder.split_once('#') {
                Some((doc, anchor)) => (doc, anchor),
                None => (remainder, ""),
            };
            // Only strip a leading '/' left over from joining onto uri_base
            // (posix_join always inserts one); an absolute-location item's
            // remainder equals the full original location and keeps its own
            // leading '/' as part of the (synthetic) docname, exercising
            // posix_join's absolute-override branch on the way back in.
            let docname = if remainder.len() != item.uri.len() {
                docname.strip_prefix('/').unwrap_or(docname)
            } else {
                docname
            };

            let dispname = if item.display_name == "-" {
                name.clone()
            } else {
                item.display_name.clone()
            };

            by_domain
                .entry(domain.to_string())
                .or_default()
                .push(InvObject {
                    name: name.clone(),
                    objtype: objtype.to_string(),
                    priority: 0,
                    docname: docname.to_string(),
                    anchor: anchor.to_string(),
                    dispname,
                });
        }
    }

    by_domain
}

#[tokio::test]
async fn writer_round_trip_matches_expected_table() {
    let manifest = load_manifest();
    let mut failures = Vec::new();

    for case in manifest.cases.iter().filter(|c| c.kind == "ok") {
        let project = case
            .project
            .as_deref()
            .unwrap_or_else(|| panic!("ok case {:?} has no `project` field", case.name));
        let version = case.version.as_deref().unwrap_or("");

        let by_domain = invobjects_from_expected_table(&case.expect, &manifest.uri_base);
        let domains: Vec<(&str, Vec<InvObject>)> = by_domain
            .iter()
            .map(|(name, objs)| (name.as_str(), objs.clone()))
            .collect();

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        InventoryFile::dump(tmp.path(), project, version, &domains, |docname: &str| {
            docname.to_string()
        })
        .await
        .unwrap_or_else(|e| panic!("case {:?}: dump failed: {e}", case.name));

        let written = std::fs::read(tmp.path()).expect("read back written inventory");
        match InventoryFile::loads(&written, &manifest.uri_base) {
            Ok(inv) => {
                let actual = inventory_to_table(&inv);
                if actual != case.expect {
                    failures.push(format!(
                        "case {:?}: writer round trip differs from expected.\n  expected: {:#?}\n  actual:   {:#?}",
                        case.name, case.expect, actual
                    ));
                }
            }
            Err(e) => failures.push(format!(
                "case {:?}: re-parsing our own dump failed: {e}",
                case.name
            )),
        }
    }

    assert!(
        failures.is_empty(),
        "{} writer round-trip mismatch(es):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

// ---------------------------------------------------------------------------
// Test 4: writer reproduces the oracle's OWN decompressed lines byte-exact,
// given the exact same pre-compaction `get_objects()` records real Sphinx
// fed its own writer (`raw_objects`, sphinx_build-sourced cases only).
// Line order follows the per-domain sort contract (`env.domains.sorted()` +
// `sorted(domain.get_objects())`), which `dump` reproduces regardless of
// the order these are handed in (grouped by domain here, but the objects
// within a domain are NOT pre-sorted -- this is what actually exercises the
// writer's own sort, not just an already-sorted passthrough).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn writer_reproduces_oracle_decompressed_lines_byte_exact() {
    let manifest = load_manifest();
    let mut failures = Vec::new();
    let mut checked = 0usize;

    for case in manifest
        .cases
        .iter()
        .filter(|c| c.kind == "ok" && c.raw_objects.is_some())
    {
        checked += 1;
        let project = case.project.as_deref().unwrap();
        let version = case.version.as_deref().unwrap();
        let raw_objects = case.raw_objects.as_ref().unwrap();

        let domains: Vec<(&str, Vec<InvObject>)> = raw_objects
            .iter()
            .map(|(domain_name, objs)| {
                let objs = objs
                    .iter()
                    .map(|o: &RawObject| InvObject {
                        name: o.name.clone(),
                        objtype: o.objtype.clone(),
                        priority: o.priority,
                        docname: o.docname.clone(),
                        anchor: o.anchor.clone(),
                        dispname: o.dispname.clone(),
                    })
                    .collect();
                (domain_name.as_str(), objs)
            })
            .collect();

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        // The corpus is built with the plain HTML builder, whose
        // `get_target_uri(docname)` is exactly `f'{docname}.html'`.
        InventoryFile::dump(tmp.path(), project, version, &domains, |docname: &str| {
            format!("{docname}.html")
        })
        .await
        .unwrap_or_else(|e| panic!("case {:?}: dump failed: {e}", case.name));

        let written = std::fs::read(tmp.path()).expect("read back written inventory");
        let oracle_raw = load_inv_bytes(&case.inv_file);

        let our_lines = decompressed_lines_from_inv_bytes(&written);
        let oracle_lines = decompressed_lines_from_inv_bytes(&oracle_raw);

        if our_lines != oracle_lines {
            failures.push(format!(
                "case {:?}: decompressed lines differ from the oracle.\n  oracle: {:#?}\n  ours:   {:#?}",
                case.name, oracle_lines, our_lines
            ));
        }
    }

    assert!(
        checked > 0,
        "no case carried raw_objects -- fixture regenerated without them?"
    );
    assert!(
        failures.is_empty(),
        "{} byte-exact line mismatch(es):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

// ---------------------------------------------------------------------------
// Test 5: binary safety -- the case the manifest flags as exercising a
// compressed tail with bare CR/LF bytes and invalid UTF-8 as a whole must
// still parse correctly. This is the exact byte class the old reader
// corrupted (`String::from_utf8_lossy` + `.lines()` over the raw zlib tail,
// pre-Task-11 `src/inventory.rs`).
// ---------------------------------------------------------------------------

#[test]
fn binary_unsafe_compressed_tail_still_parses() {
    let manifest = load_manifest();
    let case = manifest
        .cases
        .iter()
        .find(|c| c.name == manifest.unsafe_utf8_case)
        .expect("unsafe_utf8_case names a real case in the manifest");

    let raw = load_inv_bytes(&case.inv_file);
    let compressed = compressed_tail(&raw);

    assert!(
        compressed.contains(&0x0D) && compressed.contains(&0x0A),
        "case {:?} no longer contains bare CR/LF bytes in its compressed tail \
         -- the fixture corpus shrank below the binary-safety threshold",
        case.name
    );
    assert!(
        std::str::from_utf8(compressed).is_err(),
        "case {:?}'s compressed tail is valid UTF-8 -- no longer exercises \
         the binary-safety case",
        case.name
    );

    let inv = InventoryFile::loads(&raw, &manifest.uri_base)
        .unwrap_or_else(|e| panic!("case {:?}: expected Ok, got Err({e})", case.name));
    let actual = inventory_to_table(&inv);
    assert_eq!(
        actual, case.expect,
        "case {:?}: parsed table differs from expected even though loads() succeeded",
        case.name
    );
}
