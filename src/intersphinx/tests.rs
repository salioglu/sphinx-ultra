//! Every intersphinx test is offline: inventories come from the committed
//! `tests/fixtures/inventories/*.inv` bytes or are built in-process, and the
//! remote half is exercised through an injected [`InventoryFetcher`] — the
//! same approach Sphinx's own suite takes (research spec §5).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::*;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/inventories")
}

fn fixture_bytes(name: &str) -> Vec<u8> {
    std::fs::read(fixtures_dir().join(name))
        .unwrap_or_else(|e| panic!("fixture {name} must be readable: {e}"))
}

/// A version-2 inventory built from entry lines, framed exactly as
/// `InventoryFile.dump` frames one.
fn inventory_bytes(project: &str, version: &str, entries: &[&str]) -> Vec<u8> {
    let mut out = format!(
        "# Sphinx inventory version 2\n\
         # Project: {project}\n\
         # Version: {version}\n\
         # The remainder of this file is compressed using zlib.\n"
    )
    .into_bytes();
    let mut body = String::new();
    for entry in entries {
        body.push_str(entry);
        body.push('\n');
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(9));
    encoder.write_all(body.as_bytes()).unwrap();
    out.extend_from_slice(&encoder.finish().unwrap());
    out
}

#[derive(Default)]
struct MockFetcher {
    routes: BTreeMap<String, Vec<u8>>,
    calls: RefCell<Vec<String>>,
}

impl MockFetcher {
    fn with(url: &str, body: Vec<u8>) -> Self {
        Self {
            routes: BTreeMap::from([(url.to_string(), body)]),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl InventoryFetcher for MockFetcher {
    fn fetch(&self, url: &str, _http: &HttpConfig) -> anyhow::Result<Vec<u8>> {
        self.calls.borrow_mut().push(url.to_string());
        self.routes
            .get(url)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("mock has no route for {url}"))
    }
}

/// A fetcher that refuses every call — proof that a code path never went to
/// the network.
struct NoNetwork;

impl InventoryFetcher for NoNetwork {
    fn fetch(&self, url: &str, _http: &HttpConfig) -> anyhow::Result<Vec<u8>> {
        panic!("the network must not be reached; something asked for {url}")
    }
}

fn mapping(entries: &[(&str, &str, &[Option<&str>])]) -> IntersphinxMapping {
    entries
        .iter()
        .map(|(name, uri, locations)| {
            (
                (*name).to_string(),
                (
                    (*uri).to_string(),
                    locations
                        .iter()
                        .map(|l| l.map(str::to_string))
                        .collect::<Vec<_>>(),
                ),
            )
        })
        .collect()
}

fn load(
    mapping: &IntersphinxMapping,
    srcdir: &Path,
    fetcher: &dyn InventoryFetcher,
) -> LoadOutcome {
    let http = HttpConfig::default();
    load_mappings(
        &LoadRequest {
            mapping,
            srcdir,
            cache_dir: None,
            cache_limit: 5,
            now: 1_700_000_000,
            http: &http,
        },
        fetcher,
    )
}

/// A `conf.py` dict literal, through the real conf.py literal parser — the
/// only thing that ever feeds [`validate_mapping`] in a build.
fn conf_value(literal: &str) -> JsonValue {
    crate::python_config::parse_python_literal(literal)
        .unwrap_or_else(|e| panic!("the conf.py literal must parse: {e}"))
}

fn query<'a>(reftype: &'a str, reftarget: &'a str) -> XrefQuery<'a> {
    XrefQuery {
        refdomain: "std",
        reftype,
        reftarget,
        refexplicit: false,
        refdoc: "index",
        contnode_text: reftarget,
    }
}

fn isx_from(inventories: &[(&str, Vec<u8>, &str)]) -> Intersphinx {
    let mut data = IntersphinxData::default();
    for (name, bytes, uri) in inventories {
        let inventory = InventoryFile::loads(bytes, uri).expect("fixture inventory parses");
        for (objtype, objects) in &inventory.data {
            data.main
                .data
                .entry(objtype.clone())
                .or_default()
                .extend(objects.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        data.named.insert((*name).to_string(), inventory);
    }
    Intersphinx {
        data,
        disabled_reftypes: BTreeSet::from(["std:doc".to_string()]),
        resolve_self: String::new(),
    }
}

// ---------------------------------------------------------------------------
// 1. mapping validation — the eight exact texts
// ---------------------------------------------------------------------------

#[test]
fn a_well_formed_mapping_normalises_to_uri_and_locations() {
    let raw = conf_value(
        "{\n\
         'python': ('https://docs.python.org/3', None),\n\
         'local': ('https://example.org/', 'local.inv'),\n\
         'pair': ('https://two.example/', ('a.inv', None)),\n\
         }\n",
    );
    let (mapping, errors) = validate_mapping(&raw);

    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    assert_eq!(
        mapping["python"],
        ("https://docs.python.org/3".to_string(), vec![None])
    );
    assert_eq!(
        mapping["local"],
        (
            "https://example.org/".to_string(),
            vec![Some("local.inv".to_string())]
        ),
        "a bare string location is wrapped into a one-element tuple"
    );
    assert_eq!(
        mapping["pair"],
        (
            "https://two.example/".to_string(),
            vec![Some("a.inv".to_string()), None]
        )
    );
}

#[test]
fn mapping_validation_reports_sphinxs_exact_error_texts() {
    // 1: empty project identifier.
    let (_, errors) = validate_mapping(&conf_value("{'': ('https://x/', None)}\n"));
    assert_eq!(
        errors,
        vec![
            "Invalid intersphinx project identifier `''` in intersphinx_mapping. \
             Project identifiers must be non-empty strings."
        ]
    );

    // 2: value is not a two-element sequence at all.
    let (_, errors) = validate_mapping(&conf_value("{'p': 'https://x/'}\n"));
    assert_eq!(
        errors,
        vec![
            "Invalid value `'https://x/'` in intersphinx_mapping['p']. \
             Expected a two-element tuple or list."
        ]
    );

    // 3: a sequence of the wrong arity.
    let (_, errors) = validate_mapping(&conf_value("{'p': ['https://x/', None, 'extra']}\n"));
    assert_eq!(
        errors,
        vec![
            "Invalid value `['https://x/', None, 'extra']` in intersphinx_mapping['p']. \
             Values must be a (target URI, inventory locations) pair."
        ]
    );

    // 4: an empty or non-string target URI.
    let (_, errors) = validate_mapping(&conf_value("{'p': ('', None)}\n"));
    assert_eq!(
        errors,
        vec![
            "Invalid target URI value `''` in intersphinx_mapping['p'][0]. \
             Target URIs must be unique non-empty strings."
        ]
    );
    let (_, errors) = validate_mapping(&conf_value("{'p': (None, None)}\n"));
    assert_eq!(
        errors,
        vec![
            "Invalid target URI value `None` in intersphinx_mapping['p'][0]. \
             Target URIs must be unique non-empty strings."
        ]
    );

    // 5: two projects claiming the same target URI. Entries are visited in
    // key order, so 'aaa' is the one that claims it first.
    let (mapping, errors) = validate_mapping(&conf_value(
        "{'zzz': ('https://x/', None), 'aaa': ('https://x/', None)}\n",
    ));
    assert_eq!(
        errors,
        vec![
            "Invalid target URI value `'https://x/'` in intersphinx_mapping['zzz'][0]. \
             Target URIs must be unique (other instance in intersphinx_mapping['aaa'])."
        ]
    );
    assert!(mapping.contains_key("aaa") && !mapping.contains_key("zzz"));

    // 6: an inventory location that is neither None nor a non-empty string.
    let (_, errors) = validate_mapping(&conf_value("{'p': ('https://x/', ('', 'ok.inv'))}\n"));
    assert_eq!(
        errors,
        vec![
            "Invalid inventory location value `''` in intersphinx_mapping['p'][1]. \
             Inventory locations must be non-empty strings or None."
        ]
    );
    let (_, errors) = validate_mapping(&conf_value("{'p': ('https://x/', 42)}\n"));
    assert_eq!(
        errors,
        vec![
            "Invalid inventory location value `42` in intersphinx_mapping['p'][1]. \
             Inventory locations must be non-empty strings or None."
        ]
    );
}

#[test]
fn the_config_error_counts_its_errors_the_way_sphinx_does() {
    assert_eq!(
        mapping_config_error(1),
        "Invalid `intersphinx_mapping` configuration (1 error)."
    );
    assert_eq!(
        mapping_config_error(3),
        "Invalid `intersphinx_mapping` configuration (3 errors)."
    );
}

// ---------------------------------------------------------------------------
// 2. loading
// ---------------------------------------------------------------------------

#[test]
fn a_local_inventory_location_is_read_relative_to_the_source_directory() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("local.inv"),
        fixture_bytes("std_objects_and_docs.inv"),
    )
    .unwrap();

    let mapping = mapping(&[("other", "https://example.org/v1/", &[Some("local.inv")])]);
    // NoNetwork proves a local location never reaches the fetcher.
    let outcome = load(&mapping, tmp.path(), &NoNetwork);

    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert_eq!(
        outcome.data.named["other"]
            .get("std:label", "example")
            .map(|item| item.uri.as_str()),
        Some("https://example.org/v1/b.html#example"),
        "the item URI is joined against the *target* URI, not the file location"
    );
    assert_eq!(
        outcome.data.main.get("std:label", "example"),
        outcome.data.named["other"].get("std:label", "example"),
        "a single project's entries are also in the merged main inventory"
    );
    assert_eq!(
        outcome.infos.first().map(String::as_str),
        Some("loading intersphinx inventory 'other' from local.inv ...")
    );
}

#[test]
fn a_null_location_means_objects_inv_under_the_target_uri() {
    let mapping = mapping(&[("remote", "https://example.org/v1/", &[None])]);
    let fetcher = MockFetcher::with(
        "https://example.org/v1/objects.inv",
        fixture_bytes("std_objects_and_docs.inv"),
    );
    let outcome = load(&mapping, Path::new("/nonexistent"), &fetcher);

    assert_eq!(
        fetcher.calls.borrow().as_slice(),
        ["https://example.org/v1/objects.inv"]
    );
    assert!(outcome.data.inventory_exists("remote"));
}

#[test]
fn a_failing_location_falls_through_to_the_next_one() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("good.inv"),
        fixture_bytes("std_objects_and_docs.inv"),
    )
    .unwrap();

    let mapping = mapping(&[(
        "other",
        "https://example.org/v1/",
        &[Some("missing.inv"), Some("good.inv")],
    )]);
    let outcome = load(&mapping, tmp.path(), &NoNetwork);

    assert!(
        outcome.data.inventory_exists("other"),
        "the second location must be tried"
    );
    assert!(
        outcome.warnings.is_empty(),
        "a working alternative is an info, not a warning: {:?}",
        outcome.warnings
    );
    assert!(outcome.infos.iter().any(|info| info
        == "encountered some issues with some of the inventories, \
            but they had working alternatives:"));
    assert!(
        outcome.infos.iter().any(|info| info.starts_with(
            "intersphinx inventory 'missing.inv' not readable due to FileNotFoundError:"
        )),
        "infos: {:?}",
        outcome.infos
    );
}

#[test]
fn every_location_failing_is_one_warning_listing_all_of_them() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mapping = mapping(&[(
        "other",
        "https://example.org/v1/",
        &[Some("a.inv"), Some("b.inv")],
    )]);
    let outcome = load(&mapping, tmp.path(), &NoNetwork);

    assert!(outcome.data.is_empty());
    assert_eq!(outcome.warnings.len(), 1);
    let warning = &outcome.warnings[0];
    assert!(
        warning.starts_with("failed to reach any of the inventories with the following issues:\n"),
        "{warning}"
    );
    assert!(
        warning.contains("'a.inv'") && warning.contains("'b.inv'"),
        "{warning}"
    );
}

#[test]
fn a_parse_failure_is_relabelled_as_an_unsupported_inventory_version() {
    // `_load_inventory` wraps *any* ValueError from the parser as an
    // unsupported-version error, header failures included
    // (`_load.py:377-385`).
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("bad.inv"),
        fixture_bytes("err_invalid_header.inv"),
    )
    .unwrap();

    let mapping = mapping(&[("other", "https://example.org/v1/", &[Some("bad.inv")])]);
    let outcome = load(&mapping, tmp.path(), &NoNetwork);

    assert_eq!(
        outcome.warnings,
        vec![
            "failed to reach any of the inventories with the following issues:\n\
             unknown or unsupported inventory version: \
             ValueError('invalid inventory header: Not a Sphinx inventory header')"
        ]
    );
}

#[test]
fn the_disk_cache_short_circuits_a_remote_fetch_until_it_expires() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cache_dir = tmp.path().join(CACHE_DIR_NAME);
    let http = HttpConfig::default();
    let mapping = mapping(&[("remote", "https://example.org/v1/", &[None])]);
    let fetcher = MockFetcher::with(
        "https://example.org/v1/objects.inv",
        fixture_bytes("std_objects_and_docs.inv"),
    );

    let request = |now: i64, cache_limit: i64| LoadRequest {
        mapping: &mapping,
        srcdir: tmp.path(),
        cache_dir: Some(cache_dir.clone()),
        cache_limit,
        now,
        http: &http,
    };
    // The expiry basis is the cached *file's* mtime, which the filesystem
    // stamps with the real clock — so `now` has to be anchored to that, and
    // only the offsets are synthetic.
    let real_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // Cold: fetched, and the raw bytes are written to the disk cache.
    let outcome = load_mappings(&request(real_now, 5), &fetcher);
    assert!(outcome.data.inventory_exists("remote"));
    assert_eq!(fetcher.calls.borrow().len(), 1);
    assert!(cache_dir.join("remote_objects.inv").is_file());

    // Warm: the disk copy is fresh, so the fetcher is never called again.
    let outcome = load_mappings(&request(real_now, 5), &NoNetwork);
    assert!(outcome.data.inventory_exists("remote"));

    // Expired: `now` is a year on with a 5-day limit, so it re-fetches.
    let outcome = load_mappings(&request(real_now + 365 * 86400, 5), &fetcher);
    assert!(outcome.data.inventory_exists("remote"));
    assert_eq!(
        fetcher.calls.borrow().len(),
        2,
        "the stale copy is refetched"
    );

    // A negative limit never expires: back to the disk copy, no network.
    let outcome = load_mappings(&request(real_now + 365 * 86400, -1), &NoNetwork);
    assert!(outcome.data.inventory_exists("remote"));
}

#[test]
fn a_local_location_is_re_read_even_when_a_disk_cache_exists() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cache_dir = tmp.path().join(CACHE_DIR_NAME);
    std::fs::create_dir_all(&cache_dir).unwrap();
    // A disk cache entry that would satisfy a *remote* location.
    std::fs::write(
        cache_dir.join("other_objects.inv"),
        fixture_bytes("std_objects_and_docs.inv"),
    )
    .unwrap();
    // The local file says something different, and must win.
    std::fs::write(
        tmp.path().join("local.inv"),
        inventory_bytes("local", "1.0", &["only-here std:label -1 x.html#$ -"]),
    )
    .unwrap();

    let http = HttpConfig::default();
    let mapping = mapping(&[("other", "https://example.org/v1/", &[Some("local.inv")])]);
    let outcome = load_mappings(
        &LoadRequest {
            mapping: &mapping,
            srcdir: tmp.path(),
            cache_dir: Some(cache_dir),
            cache_limit: 5,
            now: 1_700_000_000,
            http: &http,
        },
        &NoNetwork,
    );

    assert!(outcome.data.named["other"].contains("std:label", "only-here"));
    assert!(
        !outcome.data.named["other"].contains("std:label", "example"),
        "the disk cache must not shadow a local file"
    );
}

#[test]
fn merged_inventories_shadow_by_name_then_expiry() {
    // Two projects define the same label; the merge sorts by (name, expiry)
    // and later entries overwrite earlier ones, so 'bbb' wins over 'aaa'.
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("a.inv"),
        inventory_bytes("A", "1", &["shared std:label -1 a.html#$ From A"]),
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("b.inv"),
        inventory_bytes("B", "1", &["shared std:label -1 b.html#$ From B"]),
    )
    .unwrap();

    let mapping = mapping(&[
        ("aaa", "https://a.example/", &[Some("a.inv")]),
        ("bbb", "https://b.example/", &[Some("b.inv")]),
    ]);
    let outcome = load(&mapping, tmp.path(), &NoNetwork);

    assert_eq!(
        outcome.data.main.get("std:label", "shared").unwrap().uri,
        "https://b.example/b.html#shared"
    );
    assert_eq!(
        outcome.data.named["aaa"]
            .get("std:label", "shared")
            .unwrap()
            .uri,
        "https://a.example/a.html#shared",
        "the per-project inventories keep their own copy"
    );
}

#[test]
fn safe_url_hides_the_password_and_leaves_everything_else_alone() {
    assert_eq!(
        safe_url("https://user:12345@example.com/objects.inv"),
        "https://user@example.com/objects.inv"
    );
    assert_eq!(
        safe_url("https://example.com/objects.inv"),
        "https://example.com/objects.inv"
    );
    assert_eq!(safe_url("local.inv"), "local.inv");
}

#[test]
fn a_redirect_only_rewrites_a_target_uri_that_pointed_at_the_inventory() {
    // The three forms Sphinx accepts as "the target URI is this inventory's
    // own directory" (`_load.py:414-419`).
    for target in [
        "https://old.example/objects.inv",
        "https://old.example",
        "https://old.example/",
    ] {
        assert_eq!(
            redirect_target_uri(
                "https://old.example/objects.inv",
                "https://new.example/objects.inv",
                target
            ),
            "https://new.example"
        );
    }
    assert_eq!(
        redirect_target_uri(
            "https://old.example/objects.inv",
            "https://new.example/objects.inv",
            "https://docs.example/somewhere-else/"
        ),
        "https://docs.example/somewhere-else/",
        "an unrelated target URI is left alone"
    );
    assert_eq!(
        redirect_target_uri(
            "https://x/objects.inv",
            "https://x/objects.inv",
            "https://x"
        ),
        "https://x",
        "no redirect, no rewrite"
    );
}

// ---------------------------------------------------------------------------
// 3. resolution
// ---------------------------------------------------------------------------

fn std_isx() -> Intersphinx {
    isx_from(&[(
        "other",
        fixture_bytes("std_objects_and_docs.inv"),
        "https://example.org/v1/",
    )])
}

#[test]
fn a_label_resolves_through_the_merged_inventory() {
    let isx = std_isx();
    let mut diagnostics = Vec::new();
    let outcome = isx.resolve_detect(&query("ref", "explicit-target"), &mut diagnostics);

    assert_eq!(
        outcome,
        HookOutcome::Resolved(Resolution {
            refuri: "https://example.org/v1/index.html#explicit-target".to_string(),
            reftitle: "(in fixture)".to_string(),
            title: Some("Explicit Target Section".to_string()),
        })
    );
    assert!(diagnostics.is_empty());
}

#[test]
fn the_disabled_reftypes_matrix_matches_sphinxs() {
    let isx = std_isx();
    let mut diagnostics = Vec::new();

    // Default `['std:doc']`: a bare `:doc:` is blocked...
    assert_eq!(
        isx.resolve_detect(&query("doc", "a"), &mut diagnostics),
        HookOutcome::Missing
    );
    // ...but the `inv:target` form bypasses disabling entirely.
    assert!(matches!(
        isx.resolve_detect(&query("doc", "other:a"), &mut diagnostics),
        HookOutcome::Resolved(_)
    ));
    // ...and so does an explicit inventory (what `:external:` uses).
    assert!(isx
        .resolve_in_inventory("other", &query("doc", "a"), &mut diagnostics)
        .is_some());
    // A reftype that is not disabled still resolves bare.
    assert!(matches!(
        isx.resolve_detect(&query("ref", "example"), &mut diagnostics),
        HookOutcome::Resolved(_)
    ));

    // `['std:*']` blocks every bare std reference.
    let mut all_std = std_isx();
    all_std.disabled_reftypes = BTreeSet::from(["std:*".to_string()]);
    assert_eq!(
        all_std.resolve_detect(&query("ref", "example"), &mut diagnostics),
        HookOutcome::Missing
    );
    assert!(matches!(
        all_std.resolve_detect(&query("ref", "other:example"), &mut diagnostics),
        HookOutcome::Resolved(_)
    ));

    // `['*']` blocks every bare reference at all.
    let mut everything = std_isx();
    everything.disabled_reftypes = BTreeSet::from(["*".to_string()]);
    assert_eq!(
        everything.resolve_detect(&query("ref", "example"), &mut diagnostics),
        HookOutcome::Missing
    );
    assert!(matches!(
        everything.resolve_detect(&query("ref", "other:example"), &mut diagnostics),
        HookOutcome::Resolved(_)
    ));

    // No disabling at all: `:doc:` resolves bare.
    let mut nothing = std_isx();
    nothing.disabled_reftypes = BTreeSet::new();
    assert!(matches!(
        nothing.resolve_detect(&query("doc", "a"), &mut diagnostics),
        HookOutcome::Resolved(_)
    ));
}

#[test]
fn an_unknown_inventory_prefix_is_simply_unresolved() {
    let isx = std_isx();
    let mut diagnostics = Vec::new();
    assert_eq!(
        isx.resolve_detect(&query("ref", "nosuchinv:example"), &mut diagnostics),
        HookOutcome::Missing
    );
    assert!(diagnostics.is_empty());
}

#[test]
fn a_target_prefixed_with_intersphinx_resolve_self_goes_back_to_the_local_domain() {
    let mut isx = std_isx();
    isx.resolve_self = "other".to_string();
    let mut diagnostics = Vec::new();

    // `other:` names *this* project, so the prefix is stripped and the
    // caller retries locally rather than resolving into the inventory.
    assert_eq!(
        isx.resolve_detect(&query("ref", "other:example"), &mut diagnostics),
        HookOutcome::SelfReferential("example".to_string())
    );
    // A target with no prefix is unaffected.
    assert!(matches!(
        isx.resolve_detect(&query("ref", "example"), &mut diagnostics),
        HookOutcome::Resolved(_)
    ));
}

#[test]
fn only_labels_and_terms_fall_back_to_a_case_insensitive_match() {
    let isx = isx_from(&[(
        "other",
        inventory_bytes(
            "fixture",
            "",
            &[
                "a term std:term -1 g.html#term-a-term -",
                "MixedLabel std:label -1 p.html#$ -",
                "MixedDoc std:doc -1 d.html -",
            ],
        ),
        "https://example.org/v1/",
    )]);
    let mut diagnostics = Vec::new();

    assert!(
        matches!(
            isx.resolve_detect(&query("term", "A TERM"), &mut diagnostics),
            HookOutcome::Resolved(_)
        ),
        "std:term folds case"
    );
    assert!(
        matches!(
            isx.resolve_detect(&query("ref", "mixedlabel"), &mut diagnostics),
            HookOutcome::Resolved(_)
        ),
        "std:label folds case"
    );
    // `std:doc` is not one of the two, so the same trick fails there — with
    // disabling switched off, so it is the case rule under test and not the
    // default `['std:doc']` block.
    let mut docs = isx.clone();
    docs.disabled_reftypes = BTreeSet::new();
    assert!(matches!(
        docs.resolve_detect(&query("doc", "MixedDoc"), &mut diagnostics),
        HookOutcome::Resolved(_)
    ));
    assert_eq!(
        docs.resolve_detect(&query("doc", "mixeddoc"), &mut diagnostics),
        HookOutcome::Missing,
        "std:doc does not fold case"
    );
    assert!(diagnostics.is_empty());
}

#[test]
fn several_case_insensitive_matches_with_different_targets_warn_once() {
    let isx = isx_from(&[(
        "other",
        inventory_bytes(
            "fixture",
            "",
            &[
                "a term std:term -1 g.html#term-a-term -",
                "A TERM std:term -1 g.html#term-A-TERM -",
            ],
        ),
        "https://example.org/v1/",
    )]);
    let mut diagnostics = Vec::new();

    // Neither spelling matches `A Term` exactly, so both are candidates.
    let outcome = isx.resolve_detect(&query("term", "A Term"), &mut diagnostics);
    assert!(matches!(outcome, HookOutcome::Resolved(_)));
    assert_eq!(
        diagnostics,
        vec![Diagnostic {
            message: "inventory 'main_inventory': multiple matches found for std:term:A Term"
                .to_string(),
            category: Some("intersphinx.external".to_string()),
        }],
        "the merged inventory calls itself 'main_inventory'"
    );

    // Inside a named inventory the descriptor is that name.
    let mut named = Vec::new();
    isx.resolve_in_inventory("other", &query("term", "A Term"), &mut named)
        .unwrap();
    assert_eq!(
        named[0].message,
        "inventory 'other': multiple matches found for std:term:A Term"
    );
}

#[test]
fn identical_duplicate_case_insensitive_matches_do_not_warn() {
    // Same priority, location and dispname on both spellings: innocuous
    // duplicates, logged at debug rather than warned.
    let isx = isx_from(&[(
        "other",
        inventory_bytes(
            "fixture",
            "",
            &[
                "a term std:term -1 g.html#same -",
                "A TERM std:term -1 g.html#same -",
            ],
        ),
        "https://example.org/v1/",
    )]);
    let mut diagnostics = Vec::new();
    assert!(matches!(
        isx.resolve_detect(&query("term", "A Term"), &mut diagnostics),
        HookOutcome::Resolved(_)
    ));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn the_std_option_and_py_attribute_objtype_shims_still_resolve() {
    // `:option:` looks for std:cmdoption, and also std:option, which is
    // where Sphinx <= 1.5 stored them.
    let isx = isx_from(&[(
        "other",
        inventory_bytes(
            "fixture",
            "",
            &[
                "--legacy std:option 1 c.html#$ -",
                "Cls.prop py:method 1 a.html#$ -",
            ],
        ),
        "https://example.org/v1/",
    )]);
    let mut diagnostics = Vec::new();

    assert!(matches!(
        isx.resolve_detect(&query("option", "--legacy"), &mut diagnostics),
        HookOutcome::Resolved(_)
    ));

    // `:attr:` looks for py:attribute, and also py:method, which is where
    // properties have lived since Sphinx 2.1.
    let mut property = query("attr", "Cls.prop");
    property.refdomain = "py";
    assert!(matches!(
        isx.resolve_detect(&property, &mut diagnostics),
        HookOutcome::Resolved(_)
    ));
}

#[test]
fn an_any_reference_sweeps_every_domain_alphabetically() {
    let isx = isx_from(&[(
        "other",
        inventory_bytes("fixture", "", &["thing py:class 1 a.html#$ -"]),
        "https://example.org/v1/",
    )]);
    let mut diagnostics = Vec::new();
    let mut any = query("any", "thing");
    any.refdomain = "";
    assert!(matches!(
        isx.resolve_detect(&any, &mut diagnostics),
        HookOutcome::Resolved(_)
    ));
}

#[test]
fn the_reftitle_only_prefixes_a_v_onto_a_numeric_version() {
    let numeric = isx_from(&[(
        "other",
        inventory_bytes("Proj", "2.5", &["l std:label -1 p.html#$ -"]),
        "https://example.org/",
    )]);
    let alpha = isx_from(&[(
        "other",
        inventory_bytes("Proj", "stable", &["l std:label -1 p.html#$ -"]),
        "https://example.org/",
    )]);
    let none = isx_from(&[(
        "other",
        inventory_bytes("Proj", "", &["l std:label -1 p.html#$ -"]),
        "https://example.org/",
    )]);

    let title = |isx: &Intersphinx| {
        let mut diagnostics = Vec::new();
        match isx.resolve_detect(&query("ref", "l"), &mut diagnostics) {
            HookOutcome::Resolved(resolution) => resolution.reftitle,
            other => panic!("expected a resolution, got {other:?}"),
        }
    };
    assert_eq!(title(&numeric), "(in Proj v2.5)");
    assert_eq!(title(&alpha), "(in Proj stable)");
    assert_eq!(title(&none), "(in Proj)");
}

#[test]
fn the_three_display_rules_decide_what_the_reference_says() {
    let isx = isx_from(&[(
        "other",
        inventory_bytes(
            "Proj",
            "1.0",
            &[
                "named std:label -1 p.html#$ A Nice Title",
                "dashed std:label -1 p.html#$ -",
                "kw std:label -1 p.html#$ Keyword Title",
            ],
        ),
        "https://example.org/",
    )]);
    let mut diagnostics = Vec::new();

    // 1. A dispname replaces the written text.
    let HookOutcome::Resolved(resolution) =
        isx.resolve_detect(&query("ref", "named"), &mut diagnostics)
    else {
        panic!("must resolve")
    };
    assert_eq!(resolution.title.as_deref(), Some("A Nice Title"));

    // 2. An explicit title wins over the dispname.
    let mut explicit = query("ref", "named");
    explicit.refexplicit = true;
    explicit.contnode_text = "My Words";
    let HookOutcome::Resolved(resolution) = isx.resolve_detect(&explicit, &mut diagnostics) else {
        panic!("must resolve")
    };
    assert_eq!(
        resolution.title, None,
        "None means: keep the content node as parsed"
    );

    // 3. A dispname of `-` keeps the written text, stripping the `inv:`
    //    prefix an `inv:target` reference left in it.
    let mut prefixed = query("ref", "other:dashed");
    prefixed.contnode_text = "other:dashed";
    let HookOutcome::Resolved(resolution) = isx.resolve_detect(&prefixed, &mut diagnostics) else {
        panic!("must resolve")
    };
    assert_eq!(resolution.title.as_deref(), Some("dashed"));

    // 3b. The same rule applies to `:keyword:` regardless of its dispname.
    let mut keyword = query("keyword", "other:kw");
    keyword.contnode_text = "other:kw";
    let HookOutcome::Resolved(resolution) = isx.resolve_detect(&keyword, &mut diagnostics) else {
        panic!("must resolve")
    };
    assert_eq!(
        resolution.title.as_deref(),
        Some("kw"),
        "a std :keyword: never takes the inventory's display name"
    );
}

#[test]
fn a_document_relative_uri_is_adjusted_for_the_referencing_documents_depth() {
    let isx = isx_from(&[(
        "other",
        inventory_bytes("Proj", "1.0", &["l std:label -1 p.html#$ -"]),
        // A relative target URI, as `('py3k', None)` produces.
        "py3k",
    )]);
    let mut diagnostics = Vec::new();

    let at_root = isx.resolve_detect(&query("ref", "l"), &mut diagnostics);
    let HookOutcome::Resolved(resolution) = at_root else {
        panic!("must resolve")
    };
    assert_eq!(resolution.refuri, "py3k/p.html#l");

    let mut nested = query("ref", "l");
    nested.refdoc = "sub/dir/page";
    let HookOutcome::Resolved(resolution) = isx.resolve_detect(&nested, &mut diagnostics) else {
        panic!("must resolve")
    };
    assert_eq!(resolution.refuri, "../../py3k/p.html#l");
}

// ---------------------------------------------------------------------------
// 4. the `:external:` role
// ---------------------------------------------------------------------------

#[test]
fn external_role_names_split_into_inventory_and_suffix() {
    assert_eq!(
        inventory_and_name_suffix("external:name"),
        Ok((None, "name"))
    );
    assert_eq!(
        inventory_and_name_suffix("external:domain:name"),
        Ok((None, "domain:name"))
    );
    assert_eq!(
        inventory_and_name_suffix("external+inv:name"),
        Ok((Some("inv"), "name"))
    );
    assert_eq!(
        inventory_and_name_suffix("external+inv:domain:name"),
        Ok((Some("inv"), "domain:name"))
    );
    // Unreachable through the dispatcher (index 8 is always `+` or `:`),
    // but the invariant is checked rather than assumed.
    assert_eq!(
        inventory_and_name_suffix("external-nope"),
        Err("Malformed :external: role name: external-nope".to_string())
    );
}

#[test]
fn a_suffix_splits_into_domain_and_role_only_up_to_one_colon() {
    assert_eq!(domain_and_role("func"), (None, Some("func")));
    assert_eq!(domain_and_role("py:func"), (Some("py"), Some("func")));
    assert_eq!(domain_and_role("a:b:c"), (None, None));
}

#[test]
fn the_dispatcher_only_claims_names_sphinxs_does() {
    assert!(is_external_role("external:ref"));
    assert!(is_external_role("external+inv:ref"));
    assert!(!is_external_role("external:"), "len must exceed 9");
    assert!(!is_external_role("external"));
    assert!(!is_external_role("externally:ref"));
}

#[test]
fn an_external_role_name_resolves_to_a_domain_and_role() {
    assert_eq!(
        external_role("external:py:func"),
        ExternalRole::Xref {
            inventory: None,
            domain: "py".to_string(),
            role: "func".to_string(),
        },
        "the domain split must not be confused by the leading `external:`"
    );
    assert_eq!(
        external_role("external+other:std:ref"),
        ExternalRole::Xref {
            inventory: Some("other".to_string()),
            domain: "std".to_string(),
            role: "ref".to_string(),
        }
    );
    assert_eq!(
        external_role("external:ref"),
        ExternalRole::Xref {
            inventory: None,
            domain: "std".to_string(),
            role: "ref".to_string(),
        },
        "no domain given: the default domain (py) has no `ref`, so std wins"
    );
    assert_eq!(
        external_role("external:func"),
        ExternalRole::Xref {
            inventory: None,
            domain: "py".to_string(),
            role: "func".to_string(),
        },
        "the default domain is tried before std"
    );
}

#[test]
fn a_malformed_external_role_reports_sphinxs_exact_warning() {
    let failed = |name: &str| match external_role(name) {
        ExternalRole::Failed(diagnostic) => diagnostic,
        other => panic!("{name} should have failed, got {other:?}"),
    };

    assert_eq!(
        failed("external:a:b:c"),
        Diagnostic {
            message: "invalid external cross-reference suffix: 'a:b:c'".to_string(),
            category: Some("intersphinx.external".to_string()),
        }
    );
    assert_eq!(
        failed("external:nosuchdomain:ref").message,
        "domain for external cross-reference not found: 'nosuchdomain'"
    );
    assert_eq!(
        failed("external:std:nosuchrole").message,
        "role for external cross-reference not found in domain 'std': 'nosuchrole'"
    );
    assert_eq!(
        failed("external:py:function").message,
        "role for external cross-reference not found in domain 'py': 'function' \
         (perhaps you meant one of: 'func', 'obj')",
        "naming an objtype instead of a role gets the roles that name it"
    );
    assert_eq!(
        failed("external:nosuchrole").message,
        "role for external cross-reference not found in domains 'py', 'std': 'nosuchrole'"
    );
    assert_eq!(
        failed("external:cmdoption").message,
        "role for external cross-reference not found in domains 'py', 'std': 'cmdoption' \
         (perhaps you meant one of: 'std:option')"
    );
}

#[test]
fn an_unknown_inventory_name_on_an_external_role_is_reported_at_resolution() {
    let isx = std_isx();
    assert_eq!(external_inventory_missing(&isx, "other"), None);
    assert_eq!(
        external_inventory_missing(&isx, "nope"),
        Some(Diagnostic {
            message: "inventory for external cross-reference not found: 'nope'".to_string(),
            category: Some("intersphinx.external".to_string()),
        })
    );

    // A self-referential inventory is never checked against the loaded set.
    let mut self_ref = std_isx();
    self_ref.resolve_self = "me".to_string();
    assert_eq!(external_inventory_missing(&self_ref, "me"), None);
}

#[test]
fn an_unresolved_external_reference_names_its_domain_and_type() {
    let mut query = query("ref", "whatever");
    query.refdomain = "std";
    assert_eq!(
        external_not_found(&query),
        Diagnostic {
            message: "external std:ref reference target not found: whatever".to_string(),
            // `type='ref', subtype=reftype` renders as `[ref.ref]`.
            category: Some("ref.ref".to_string()),
        }
    );
}
