//! `sphinx.ext.intersphinx` — cross-project references resolved through
//! other projects' `objects.inv` inventories.
//!
//! Ported from Sphinx 9.1.0's `sphinx/ext/intersphinx/` (`_load.py`,
//! `_resolve.py`, `_shared.py`); every message text, ordering rule and
//! fallback below is cited to the file:line it mirrors, collected in
//! `docs/superpowers/plans/2026-08-31-m2-wave4-research-spec-inventory-intersphinx.md`
//! §3-§4.
//!
//! The three phases, in the order a build runs them:
//!
//! 1. **Validation** ([`validate_mapping`]) normalises `intersphinx_mapping`
//!    at configuration load. Any failure is a `ConfigError` in Sphinx, which
//!    aborts the build — here it aborts config loading, which the CLI turns
//!    into exit code 2.
//! 2. **Loading** ([`load_mappings`]) reads each project's inventory: local
//!    files always, remote ones through an [`InventoryFetcher`] with an
//!    on-disk cache. The merged "main" inventory plus the per-name ones are
//!    the [`IntersphinxData`].
//! 3. **Resolution** ([`Intersphinx::resolve_detect`] and friends) runs from
//!    the reference resolver, standing exactly where Sphinx's
//!    `missing-reference` event does: after the local domain has failed and
//!    before the dangling-reference warning.

pub mod fetch;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use crate::env::toctree::py_repr_str;
use crate::inventory::{posix_join, Inventory, InventoryFile, InventoryItem};

pub use fetch::{HttpConfig, InventoryFetcher, TlsCacerts, UreqFetcher, DEFAULT_USER_AGENT};

/// The file name intersphinx appends to a target URI when a mapping gives no
/// explicit inventory location (`INVENTORY_FILENAME`, `_load.py:262-267`).
const INVENTORY_FILENAME: &str = "objects.inv";

/// The on-disk cache directory, relative to the doctree/cache directory
/// (`app.doctreedir / '__intersphinx_cache__'`, `_load.py:186-190`). Sphinx
/// documents that "the location of this cache directory must not be relied
/// upon externally"; ours rides the fingerprint-wiped cache dir, so a
/// configuration change clears it along with everything else.
pub const CACHE_DIR_NAME: &str = "__intersphinx_cache__";

/// The normalised `intersphinx_mapping`: project name -> (target URI,
/// inventory locations). Sphinx stores `{name: (name, (uri, locations))}`
/// with the name duplicated inside the value (`_load.py:129`); the key is
/// the same string, so this drops the duplicate.
///
/// A location of `None` means "`objects.inv` under the target URI".
pub type IntersphinxMapping = BTreeMap<String, (String, Vec<Option<String>>)>;

// ---------------------------------------------------------------------------
// 1. Mapping validation (`validate_intersphinx_mapping`, `_load.py:38-136`)
// ---------------------------------------------------------------------------

/// Validate and normalise a raw `intersphinx_mapping` value as parsed from
/// `conf.py`, returning the entries that survived and the error messages for
/// the ones that did not.
///
/// The messages are Sphinx's own, verbatim (`_load.py:59-125`, checks 1-6 in
/// the research spec §3). Sphinx logs each with `LOGGER.error` and then
/// raises `ConfigError` if any fired; [`mapping_config_error`] is that final
/// message.
///
/// Two knowing divergences, both forced by going through JSON rather than
/// live Python objects:
///
/// * Entries are visited in *key order*, not `conf.py` order, because the
///   parsed dict is a sorted map. This only shows in check 5, which names
///   the other entry that claimed a duplicate target URI.
/// * `%r` of a sequence renders with list brackets, since a `conf.py` tuple
///   and list both arrive as a JSON array. Checks 2, 3 and 6 can therefore
///   print `['a', 'b', 'c']` where Sphinx prints `('a', 'b', 'c')`.
pub fn validate_mapping(raw: &JsonValue) -> (IntersphinxMapping, Vec<String>) {
    let mut mapping = IntersphinxMapping::new();
    let mut errors = Vec::new();
    let Some(entries) = raw.as_object() else {
        // Sphinx's config machinery guarantees a dict here; anything else
        // would be an AttributeError inside `validate_intersphinx_mapping`.
        // Reporting nothing is the honest answer: we have no message of
        // Sphinx's to reuse, and inventing one would be worse than treating
        // the setting as absent.
        return (mapping, errors);
    };

    // uri -> the name that claimed it first (`seen`, `_load.py:53`).
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();

    for (name, value) in entries {
        if name.is_empty() {
            errors.push(format!(
                "Invalid intersphinx project identifier `{}` in intersphinx_mapping. \
                 Project identifiers must be non-empty strings.",
                py_repr_value(&JsonValue::String(name.clone()))
            ));
            continue;
        }

        let Some(pair) = value.as_array() else {
            errors.push(format!(
                "Invalid value `{}` in intersphinx_mapping[{}]. \
                 Expected a two-element tuple or list.",
                py_repr_value(value),
                py_repr_str(name)
            ));
            continue;
        };
        if pair.len() != 2 {
            errors.push(format!(
                "Invalid value `{}` in intersphinx_mapping[{}]. \
                 Values must be a (target URI, inventory locations) pair.",
                py_repr_value(value),
                py_repr_str(name)
            ));
            continue;
        }
        let (uri, inv) = (&pair[0], &pair[1]);

        let uri = match uri.as_str() {
            Some(uri) if !uri.is_empty() => uri,
            _ => {
                errors.push(format!(
                    "Invalid target URI value `{}` in intersphinx_mapping[{}][0]. \
                     Target URIs must be unique non-empty strings.",
                    py_repr_value(uri),
                    py_repr_str(name)
                ));
                continue;
            }
        };
        if let Some(other) = seen.get(uri) {
            errors.push(format!(
                "Invalid target URI value `{}` in intersphinx_mapping[{}][0]. \
                 Target URIs must be unique (other instance in intersphinx_mapping[{}]).",
                py_repr_str(uri),
                py_repr_str(name),
                py_repr_str(other)
            ));
            continue;
        }
        seen.insert(uri, name);

        // `if not isinstance(inv, (tuple, list)): inv = (inv,)`.
        let locations: Vec<&JsonValue> = match inv.as_array() {
            Some(items) => items.iter().collect(),
            None => vec![inv],
        };
        let mut targets = Vec::with_capacity(locations.len());
        for location in locations {
            match location {
                JsonValue::Null => targets.push(None),
                JsonValue::String(s) if !s.is_empty() => targets.push(Some(s.clone())),
                other => errors.push(format!(
                    "Invalid inventory location value `{}` in intersphinx_mapping[{}][1]. \
                     Inventory locations must be non-empty strings or None.",
                    py_repr_value(other),
                    py_repr_str(name)
                )),
            }
        }

        // Sphinx's `continue` here only leaves the *inner* loop, so the
        // entry is re-added with whatever targets did validate even though
        // it was just deleted (`_load.py:113-129`). Faithfully reproduced —
        // it is unobservable anyway, since any error aborts the build.
        mapping.insert(name.clone(), (uri.to_string(), targets));
    }

    (mapping, errors)
}

/// The `ConfigError` Sphinx raises once validation has logged its errors
/// (`_load.py:131-136`).
pub fn mapping_config_error(errors: usize) -> String {
    if errors == 1 {
        "Invalid `intersphinx_mapping` configuration (1 error).".to_string()
    } else {
        format!("Invalid `intersphinx_mapping` configuration ({errors} errors).")
    }
}

/// Python's `repr()` for the JSON shapes a `conf.py` literal can produce.
/// Sequences render as lists — see [`validate_mapping`]'s doc comment for
/// why a tuple cannot be told apart here.
fn py_repr_value(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "None".to_string(),
        JsonValue::Bool(true) => "True".to_string(),
        JsonValue::Bool(false) => "False".to_string(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::String(s) => py_repr_str(s),
        JsonValue::Array(items) => {
            let inner: Vec<String> = items.iter().map(py_repr_value).collect();
            format!("[{}]", inner.join(", "))
        }
        JsonValue::Object(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", py_repr_str(k), py_repr_value(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Loading (`load_mappings` / `_fetch_inventory_group`, `_load.py:139-335`)
// ---------------------------------------------------------------------------

/// The loaded inventories: the merged "main" one every un-named lookup goes
/// through, and the per-project ones an `inv:target` or `:external+inv:`
/// reference names (`_shared.py:114-149`, `InventoryAdapter`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IntersphinxData {
    pub main: Inventory,
    pub named: BTreeMap<String, Inventory>,
}

impl IntersphinxData {
    pub fn is_empty(&self) -> bool {
        self.named.is_empty()
    }

    /// `inventory_exists(env, inv_name)` (`_resolve.py:255-256`).
    pub fn inventory_exists(&self, name: &str) -> bool {
        self.named.contains_key(name)
    }
}

/// Everything [`load_mappings`] needs from the build.
pub struct LoadRequest<'a> {
    pub mapping: &'a IntersphinxMapping,
    /// Local inventory locations resolve against the *source* directory
    /// (`srcdir / inv_location`, `_load.py:424-427`).
    pub srcdir: &'a Path,
    /// `<cache_dir>/__intersphinx_cache__`, or `None` to disable the disk
    /// cache entirely (Sphinx passes `None` from `fetch_inventory`).
    pub cache_dir: Option<PathBuf>,
    /// `intersphinx_cache_limit`, in days. Negative means never expire
    /// (`_load.py:250-257`).
    pub cache_limit: i64,
    /// `int(time.time())`, injectable so cache-expiry behaviour is testable.
    pub now: i64,
    pub http: &'a HttpConfig,
}

/// What loading produced, plus the diagnostics it wants reported.
#[derive(Debug, Default)]
pub struct LoadOutcome {
    pub data: IntersphinxData,
    /// `LOGGER.warning` messages — the all-locations-failed report
    /// (`_load.py:330-334`). Logged without a `type`, so they render with no
    /// `[category]` suffix.
    pub warnings: Vec<String>,
    /// `LOGGER.info` messages, in the order Sphinx emits them.
    pub infos: Vec<String>,
}

/// Read every configured inventory (`load_mappings`, `_load.py:139-208`).
///
/// **Cache design.** Sphinx keeps `env.intersphinx_cache` — a pickled
/// `{uri: (name, expiry, inventory)}` — on the environment, so a warm
/// incremental build can skip both the download *and* the parse. This port
/// keeps only the on-disk half: `__intersphinx_cache__/{name}_objects.inv`
/// holds the raw bytes, and the file's mtime is the expiry basis, exactly as
/// Sphinx's disk short-circuit uses it (`_load.py:274-287`). A warm rebuild
/// therefore still skips the download and only re-parses, which costs
/// milliseconds — while keeping `BuildEnvironment` free of a
/// non-`serde`-shaped field that would have to version-lock with `env.bin`.
/// The in-memory map below is per-call, and exists so the merge order and
/// the cache-hit checks stay byte-faithful to Sphinx's.
pub fn load_mappings(request: &LoadRequest<'_>, fetcher: &dyn InventoryFetcher) -> LoadOutcome {
    let mut outcome = LoadOutcome::default();
    if request.mapping.is_empty() {
        return outcome;
    }

    // uri -> (name, expiry, inventory) — Sphinx's `intersphinx_cache`.
    let mut cache: Vec<(String, i64, Inventory)> = Vec::new();

    for (name, (target_uri, locations)) in request.mapping {
        fetch_inventory_group(
            request,
            fetcher,
            name,
            target_uri,
            locations,
            &mut cache,
            &mut outcome,
        );
    }

    // "Duplicate values in different inventories will shadow each other" —
    // sorted by `(name, expiry)` so the winner is at least deterministic
    // (`_load.py:196-208`). Later entries shadow earlier ones.
    cache.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    for (name, _expiry, inventory) in cache {
        for (objtype, objects) in &inventory.data {
            outcome
                .data
                .main
                .data
                .entry(objtype.clone())
                .or_default()
                .extend(objects.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        outcome.data.named.insert(name, inventory);
    }

    outcome
}

#[allow(clippy::too_many_arguments)]
fn fetch_inventory_group(
    request: &LoadRequest<'_>,
    fetcher: &dyn InventoryFetcher,
    name: &str,
    target_uri: &str,
    locations: &[Option<String>],
    cache: &mut Vec<(String, i64, Inventory)>,
    outcome: &mut LoadOutcome,
) {
    // A positive limit expires the cache `limit` days back; a negative one
    // never expires it (`_load.py:250-257`).
    let cache_time = if request.cache_limit >= 0 {
        request.now - request.cache_limit * 86400
    } else {
        0
    };

    let cache_path = request
        .cache_dir
        .as_ref()
        .map(|dir| dir.join(format!("{name}_{INVENTORY_FILENAME}")));

    let mut failures: Vec<String> = Vec::new();
    // Sphinx's `project.locations` is never empty: a mapping with no
    // locations normalises to `(None,)`.
    let locations: Vec<Option<String>> = if locations.is_empty() {
        vec![None]
    } else {
        locations.to_vec()
    };

    for location in &locations {
        let inv_location = match location {
            Some(location) => location.clone(),
            None => posix_join(target_uri, INVENTORY_FILENAME),
        };
        let remote = inv_location.contains("://");

        // Disk-cache short-circuit: remote locations only, and only while
        // the saved copy is younger than the expiry (`_load.py:274-287`).
        if let Some(cache_path) = cache_path.as_ref().filter(|_| remote) {
            if let Some(mtime) = file_mtime(cache_path) {
                if mtime >= cache_time {
                    match std::fs::read(cache_path)
                        .map_err(|e| read_failure(&inv_location, &e))
                        .and_then(|raw| load_inventory(&raw, target_uri))
                    {
                        Ok(inventory) => {
                            cache.push((name.to_string(), mtime, inventory));
                            break;
                        }
                        Err(message) => failures.push(message),
                    }
                }
            }
        }

        // Local files are always re-read; remote ones only when the cache
        // has expired (`_load.py:289-295`).
        outcome.infos.push(format!(
            "loading intersphinx inventory '{name}' from {} ...",
            safe_url(&inv_location)
        ));
        let fetched = if remote {
            fetcher
                .fetch(&inv_location, request.http)
                .map_err(|e| fetch_failure(&inv_location, &e))
                .inspect(|raw| {
                    if let Some(cache_path) = cache_path.as_ref() {
                        write_disk_cache(cache_path, raw);
                    }
                })
        } else {
            let path = request.srcdir.join(&inv_location);
            std::fs::read(&path).map_err(|e| read_failure(&inv_location, &e))
        };

        match fetched.and_then(|raw| load_inventory(&raw, target_uri)) {
            Ok(inventory) => {
                cache.push((name.to_string(), request.now, inventory));
                break;
            }
            Err(message) => failures.push(message),
        }
    }

    // Sphinx reports whatever failed even when a later location worked
    // (`_load.py:319-334`): all-failed is one warning, some-failed with a
    // working alternative is a set of infos.
    if failures.is_empty() {
    } else if failures.len() < locations.len() {
        outcome.infos.push(
            "encountered some issues with some of the inventories, \
             but they had working alternatives:"
                .to_string(),
        );
        outcome.infos.extend(failures);
    } else {
        outcome.warnings.push(format!(
            "failed to reach any of the inventories with the following issues:\n{}",
            failures.join("\n")
        ));
    }
}

/// `_load_inventory` (`_load.py:377-385`): **any** parse failure is
/// re-labelled as an unsupported-version error, wrapping the original
/// `ValueError`'s repr — even when the original was a header error.
fn load_inventory(raw: &[u8], target_uri: &str) -> Result<Inventory, String> {
    InventoryFile::loads(raw, target_uri).map_err(|err| {
        format!(
            "unknown or unsupported inventory version: ValueError({})",
            py_repr_str(&err.to_string())
        )
    })
}

/// `_fetch_inventory_file`'s error rewrite (`_load.py:428-435`).
///
/// The `%s: %s` halves are Python's exception class *name* and `str(err)`,
/// which have no exact Rust equivalent; the class name is mapped from the
/// I/O error kind and the message is Rust's. The first `%r` — the inventory
/// location, which is what makes the message useful — is exact.
fn read_failure(inv_location: &str, err: &std::io::Error) -> String {
    let class = match err.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        std::io::ErrorKind::IsADirectory => "IsADirectoryError",
        _ => "OSError",
    };
    format!(
        "intersphinx inventory {} not readable due to {class}: {err}",
        py_repr_str(inv_location)
    )
}

/// `_fetch_inventory_url`'s error rewrite (`_load.py:401-408`). Sphinx
/// interpolates `err.__class__` — the class *repr*, `<class 'x.Y'>`, not its
/// name — which has no Rust counterpart; ours names the transport instead.
fn fetch_failure(inv_location: &str, err: &anyhow::Error) -> String {
    format!(
        "intersphinx inventory {} not fetchable due to <class 'ureq.Error'>: {err}",
        py_repr_str(inv_location)
    )
}

fn write_disk_cache(cache_path: &Path, raw: &[u8]) {
    if let Some(parent) = cache_path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    // A cache that cannot be written is not a build failure; Sphinx would
    // raise here, but losing the *cache* must never lose the *build*.
    if let Err(e) = std::fs::write(cache_path, raw) {
        log::debug!(
            "could not write intersphinx disk cache {}: {e}",
            cache_path.display()
        );
    }
}

/// The file's mtime in whole seconds since the epoch, or `None` if it is not
/// a readable regular file (`cache_path.is_file()` + `stat().st_mtime`).
fn file_mtime(path: &Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let modified = meta.modified().ok()?;
    match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(since) => Some(since.as_secs() as i64),
        Err(before) => Some(-(before.duration().as_secs() as i64)),
    }
}

/// `_get_safe_url` (`_load.py:439-461`): the password is dropped from a
/// `user:password@host` URL before it reaches a log line.
pub fn safe_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let (netloc, tail) = match rest.find(['/', '?', '#']) {
        Some(end) => (&rest[..end], &rest[end..]),
        None => (rest, ""),
    };
    let Some((userinfo, host)) = netloc.rsplit_once('@') else {
        return url.to_string();
    };
    let user = userinfo.split_once(':').map_or(userinfo, |(user, _)| user);
    format!("{scheme}://{user}@{host}{tail}")
}

/// The redirect rule (`_load.py:410-419`), kept pure: after a fetch that
/// ended somewhere other than where it started, the *target* URI is rewritten
/// only when it was pointing at the inventory's own directory.
///
/// Not reachable in production — [`InventoryFetcher`] returns bytes, not the
/// final URL — but the rule is pinned here so wiring a redirect-aware
/// fetcher later is a change of plumbing, not of policy.
pub fn redirect_target_uri(inv_location: &str, new_inv_location: &str, target_uri: &str) -> String {
    if inv_location == new_inv_location {
        return target_uri.to_string();
    }
    let dirname = posix_dirname(inv_location);
    if target_uri == inv_location || target_uri == dirname || target_uri == format!("{dirname}/") {
        return posix_dirname(new_inv_location);
    }
    target_uri.to_string()
}

/// `os.path.dirname` on a `/`-separated string.
fn posix_dirname(path: &str) -> String {
    match path.rfind('/') {
        // `os.path.dirname('/x')` is `'/'`, not `''`.
        Some(0) => "/".to_string(),
        Some(idx) => path[..idx].to_string(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// 3. Domain tables
// ---------------------------------------------------------------------------

/// One domain's cross-reference surface, as intersphinx reads it off a
/// `Domain`: `object_types` (objtype -> the roles that can name it) and
/// `roles`.
struct DomainSpec {
    name: &'static str,
    /// In declaration order — `objtypes_for_role` preserves it
    /// (`domains/__init__.py:130-135` builds `_role2type` by iterating
    /// `object_types`), and so does the `any` role's objtype sweep.
    object_types: &'static [(&'static str, &'static [&'static str])],
    roles: &'static [&'static str],
}

/// The domains this build knows, in `domains.sorted()` order (alphabetical
/// by name, `domains/_domains_container.py:284-286`).
///
/// Only `py` and `std` are modelled: those are the domains sphinx-ultra
/// itself populates, and they are the ones whose objects appear in the
/// inventories real projects publish. A reference into any other domain
/// (`c`, `cpp`, `js`, `rst`, `math`) is reported as unregistered, which is
/// what `_resolve_reference` does for a domain that is not installed.
const DOMAINS: &[DomainSpec] = &[
    DomainSpec {
        name: "py",
        // `domains/python/__init__.py:725-737`.
        object_types: &[
            ("function", &["func", "obj"]),
            ("data", &["data", "obj"]),
            ("class", &["class", "exc", "obj"]),
            ("exception", &["exc", "class", "obj"]),
            ("method", &["meth", "obj"]),
            ("classmethod", &["meth", "obj"]),
            ("staticmethod", &["meth", "obj"]),
            ("attribute", &["attr", "obj"]),
            ("property", &["attr", "_prop", "obj"]),
            ("type", &["type", "class", "obj"]),
            ("module", &["mod", "obj"]),
        ],
        // `domains/python/__init__.py:755-767`.
        roles: &[
            "attr", "class", "const", "data", "deco", "exc", "func", "meth", "mod", "obj", "type",
        ],
    },
    DomainSpec {
        name: "std",
        // `domains/std/__init__.py:729-737`.
        object_types: &[
            ("term", &["term"]),
            ("token", &["token"]),
            ("label", &["ref", "keyword"]),
            ("confval", &["confval"]),
            ("envvar", &["envvar"]),
            ("cmdoption", &["option"]),
            ("doc", &["doc"]),
        ],
        // `domains/std/__init__.py:748-766`.
        roles: &[
            "confval", "doc", "envvar", "keyword", "numref", "option", "ref", "term", "token",
        ],
    },
];

fn domain(name: &str) -> Option<&'static DomainSpec> {
    DOMAINS.iter().find(|domain| domain.name == name)
}

impl DomainSpec {
    /// `domain.objtypes_for_role(role)` — every objtype the role can name,
    /// in `object_types` declaration order.
    fn objtypes_for_role(&self, role: &str) -> Vec<&'static str> {
        self.object_types
            .iter()
            .filter(|(_, roles)| roles.contains(&role))
            .map(|(objtype, _)| *objtype)
            .collect()
    }

    fn has_role(&self, role: &str) -> bool {
        self.roles.contains(&role)
    }

    /// The roles that name `objtype`, for the "perhaps you meant one of"
    /// hint (`_resolve.py:415-424`).
    fn roles_for_objtype(&self, objtype: &str) -> Option<&'static [&'static str]> {
        self.object_types
            .iter()
            .find(|(name, _)| *name == objtype)
            .map(|(_, roles)| *roles)
    }
}

// ---------------------------------------------------------------------------
// 4. Resolution (`_resolve.py:36-347`)
// ---------------------------------------------------------------------------

/// The `pending_xref` attributes resolution reads.
#[derive(Debug, Clone)]
pub struct XrefQuery<'a> {
    pub refdomain: &'a str,
    pub reftype: &'a str,
    pub reftarget: &'a str,
    pub refexplicit: bool,
    /// The document the reference was written in, which a document-relative
    /// inventory URI is adjusted against (`_resolve.py:43-46`).
    pub refdoc: &'a str,
    /// `contnode.astext()`.
    pub contnode_text: &'a str,
}

/// A reference into another project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub refuri: String,
    /// The hover title, `(in Project vX)` (`_resolve.py:47-55`).
    pub reftitle: String,
    /// `None` keeps the content node as parsed; `Some` replaces its text
    /// (`_resolve.py:57-77`).
    pub title: Option<String>,
}

/// A diagnostic resolution wants logged, with the `type.subtype` category
/// Sphinx gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
    pub category: Option<String>,
}

impl Diagnostic {
    /// `type='intersphinx', subtype='external'` — every message the
    /// inventory lookups and the `:external:` role raise.
    fn external(message: String) -> Self {
        Self {
            message,
            category: Some("intersphinx.external".to_string()),
        }
    }
}

/// What the missing-reference hook decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    /// Resolved into another project.
    Resolved(Resolution),
    /// The target named `intersphinx_resolve_self`: the reference points at
    /// *this* project, and the caller must retry the local domain with the
    /// carried target (`_resolve.py:326-333` +
    /// `post_transforms/__init__.py:140-154`).
    SelfReferential(String),
    /// Nothing matched; the caller proceeds to its dangling warning.
    Missing,
}

/// The loaded inventories plus the two configuration values resolution
/// consults.
#[derive(Debug, Clone, Default)]
pub struct Intersphinx {
    pub data: IntersphinxData,
    /// `intersphinx_disabled_reftypes`, default `['std:doc']`.
    pub disabled_reftypes: BTreeSet<String>,
    /// `intersphinx_resolve_self`, default `''` (disabled).
    pub resolve_self: String,
}

impl Intersphinx {
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// `resolve_reference_detect_inventory` (`_resolve.py:305-340`) — what
    /// the `missing-reference` event calls.
    ///
    /// Tries the merged inventory with the target as written, then splits
    /// the target on its first `:` into `inv_name:target` and retries inside
    /// that named inventory. The prefixed form deliberately bypasses
    /// `intersphinx_disabled_reftypes`.
    pub fn resolve_detect(
        &self,
        query: &XrefQuery<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> HookOutcome {
        if let Some(resolution) = self.resolve_any(true, query, diagnostics) {
            return HookOutcome::Resolved(resolution);
        }
        let Some((inv_name, new_target)) = query.reftarget.split_once(':') else {
            return HookOutcome::Missing;
        };
        if !self.resolve_self.is_empty() && self.resolve_self == inv_name {
            return HookOutcome::SelfReferential(new_target.to_string());
        }
        if !self.data.inventory_exists(inv_name) {
            return HookOutcome::Missing;
        }
        // The target is rewritten for the lookup and restored afterwards,
        // which is why the dangling warning still names the written target.
        let prefixed = XrefQuery {
            reftarget: new_target,
            ..query.clone()
        };
        match self.resolve_in_inventory(inv_name, &prefixed, diagnostics) {
            Some(resolution) => HookOutcome::Resolved(resolution),
            None => HookOutcome::Missing,
        }
    }

    /// `resolve_reference_in_inventory` (`_resolve.py:258-277`): an explicit
    /// inventory never honours the disabled reftypes.
    pub fn resolve_in_inventory(
        &self,
        inv_name: &str,
        query: &XrefQuery<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<Resolution> {
        let inventory = self.data.named.get(inv_name)?;
        self.resolve_reference(Some(inv_name), inventory, false, query, diagnostics)
    }

    /// `resolve_reference_any_inventory` (`_resolve.py:280-302`).
    pub fn resolve_any(
        &self,
        honor_disabled: bool,
        query: &XrefQuery<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<Resolution> {
        self.resolve_reference(None, &self.data.main, honor_disabled, query, diagnostics)
    }

    /// `_resolve_reference` (`_resolve.py:186-253`).
    fn resolve_reference(
        &self,
        inv_name: Option<&str>,
        inventory: &Inventory,
        honor_disabled: bool,
        query: &XrefQuery<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<Resolution> {
        // "disabling should only be done if no inventory is given".
        let honor_disabled = honor_disabled && inv_name.is_none();
        if honor_disabled && self.disabled_reftypes.contains("*") {
            return None;
        }

        if query.reftype == "any" {
            for spec in DOMAINS {
                if honor_disabled && self.disabled_reftypes.contains(&format!("{}:*", spec.name)) {
                    continue;
                }
                let objtypes: Vec<&str> = spec
                    .object_types
                    .iter()
                    .map(|(objtype, _)| *objtype)
                    .collect();
                if let Some(resolution) = self.resolve_reference_in_domain(
                    inv_name,
                    inventory,
                    honor_disabled,
                    spec,
                    &objtypes,
                    query,
                    diagnostics,
                ) {
                    return Some(resolution);
                }
            }
            return None;
        }

        if query.refdomain.is_empty() {
            // Only objects in domains are in the inventory.
            return None;
        }
        if honor_disabled
            && self
                .disabled_reftypes
                .contains(&format!("{}:*", query.refdomain))
        {
            return None;
        }
        // Sphinx raises `ExtensionError('Domain %r is not registered')` for
        // an unknown domain. Ours cannot: a reference into a domain this
        // build does not implement is an everyday occurrence here, not a
        // configuration bug, so it simply does not resolve.
        let spec = domain(query.refdomain)?;
        let objtypes = spec.objtypes_for_role(query.reftype);
        if objtypes.is_empty() {
            return None;
        }
        self.resolve_reference_in_domain(
            inv_name,
            inventory,
            honor_disabled,
            spec,
            &objtypes,
            query,
            diagnostics,
        )
    }

    /// `_resolve_reference_in_domain` (`_resolve.py:136-191`), including the
    /// two backwards-compatibility objtype shims.
    #[allow(clippy::too_many_arguments)]
    fn resolve_reference_in_domain(
        &self,
        inv_name: Option<&str>,
        inventory: &Inventory,
        honor_disabled: bool,
        spec: &DomainSpec,
        objtypes: &[&str],
        query: &XrefQuery<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<Resolution> {
        // An insertion-ordered set: `dict.fromkeys(objtypes)` with the two
        // compatibility additions appended.
        let mut obj_types: Vec<String> = Vec::with_capacity(objtypes.len() + 1);
        for objtype in objtypes {
            if !obj_types.iter().any(|existing| existing == objtype) {
                obj_types.push((*objtype).to_string());
            }
        }
        // "cmdoptions were stored as std:option until Sphinx 1.6".
        if spec.name == "std" && objtypes.contains(&"cmdoption") {
            obj_types.push("option".to_string());
        }
        // "properties are stored as py:method since Sphinx 2.1".
        if spec.name == "py" && objtypes.contains(&"attribute") {
            obj_types.push("method".to_string());
        }

        let objtypes: Vec<String> = obj_types
            .into_iter()
            .map(|objtype| format!("{}:{objtype}", spec.name))
            // The individually disabled entries go last, once the list is
            // complete and prefixed.
            .filter(|objtype| !honor_disabled || !self.disabled_reftypes.contains(objtype))
            .collect();

        // `domain.get_full_qualified_name(node)` — the module/class-scoped
        // retry — is not modelled: the std domain returns None for it, and
        // the py domain needs the `py:module`/`py:class` scope this build
        // does not carry through resolution yet.
        self.resolve_by_target(
            inv_name,
            inventory,
            spec.name,
            &objtypes,
            query.reftarget,
            query,
            diagnostics,
        )
    }

    /// `_resolve_reference_in_domain_by_target` (`_resolve.py:80-133`).
    #[allow(clippy::too_many_arguments)]
    fn resolve_by_target(
        &self,
        inv_name: Option<&str>,
        inventory: &Inventory,
        domain_name: &str,
        objtypes: &[String],
        target: &str,
        query: &XrefQuery<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<Resolution> {
        for objtype in objtypes {
            let Some(objects) = inventory.data.get(objtype.as_str()) else {
                continue;
            };
            let item = if let Some(item) = objects.get(target) {
                item
            } else if objtype == "std:label" || objtype == "std:term" {
                // Case-insensitive fallback, for these two objtypes only
                // (sphinx-doc/sphinx#9291 and #12008).
                let lowered = target.to_lowercase();
                let matches: Vec<&String> = objects
                    .keys()
                    .filter(|key| key.to_lowercase() == lowered)
                    .collect();
                if matches.len() > 1 {
                    let distinct: std::collections::HashSet<&InventoryItem> =
                        matches.iter().map(|key| &objects[*key]).collect();
                    let descriptor = inv_name.unwrap_or("main_inventory");
                    if distinct.len() == 1 {
                        log::debug!(
                            "inventory '{descriptor}': duplicate matches found for {objtype}:{target}"
                        );
                    } else {
                        diagnostics.push(Diagnostic::external(format!(
                            "inventory '{descriptor}': multiple matches found for {objtype}:{target}"
                        )));
                    }
                }
                match matches.first() {
                    Some(key) => &objects[*key],
                    None => continue,
                }
            } else {
                // A case-insensitive match for any other objtype is
                // deliberately *not* used.
                continue;
            };
            return Some(element_from_result(domain_name, inv_name, item, query));
        }
        None
    }
}

/// `_create_element_from_result` (`_resolve.py:36-77`) — the URI adjustment
/// and the three display rules.
fn element_from_result(
    domain_name: &str,
    inv_name: Option<&str>,
    item: &InventoryItem,
    query: &XrefQuery<'_>,
) -> Resolution {
    let mut uri = item.uri.clone();
    if !uri.contains("://") && !query.refdoc.is_empty() {
        // `(_relative_path(Path(), Path(refdoc).parent) / uri).as_posix()`:
        // one `..` per directory the referencing document sits in.
        let depth = query.refdoc.split('/').count().saturating_sub(1);
        if depth > 0 {
            uri = format!("{}{uri}", "../".repeat(depth));
        }
    }

    let reftitle = if item.project_version.is_empty() {
        format!("(in {})", item.project_name)
    } else {
        // A version starting with a digit gets a `v` prefix; anything else
        // is printed as written.
        let version = if item
            .project_version
            .starts_with(|c: char| c.is_ascii_digit())
        {
            format!("v{}", item.project_version)
        } else {
            item.project_version.clone()
        };
        format!("(in {} {version})", item.project_name)
    };

    let title = if query.refexplicit {
        // An explicit title wins outright.
        None
    } else if item.display_name == "-" || (domain_name == "std" && query.reftype == "keyword") {
        // Keep the written title, minus any `inv:` prefix it still carries
        // from an `inv:target` reference.
        match inv_name {
            Some(inv_name) => query
                .contnode_text
                .strip_prefix(&format!("{inv_name}:"))
                .map(str::to_string),
            None => None,
        }
    } else {
        Some(item.display_name.clone())
    };

    Resolution {
        refuri: uri,
        reftitle,
        title,
    }
}

// ---------------------------------------------------------------------------
// 5. The `:external:` role (`_resolve.py:350-533`)
// ---------------------------------------------------------------------------

/// Sphinx's `primary_domain` default, which is what
/// `env.current_document.default_domain` holds for a document with no
/// `.. default-domain::` (`sphinx/config.py`, `primary_domain = 'py'`).
const DEFAULT_DOMAIN: &str = "py";

/// Whether a role name is one `IntersphinxDispatcher` claims
/// (`_resolve.py:358-366`). The name is the one the author *wrote*: the
/// inventory name inside it is case-sensitive.
pub fn is_external_role(name: &str) -> bool {
    name.len() > 9 && (name.starts_with("external:") || name.starts_with("external+"))
}

/// `get_inventory_and_name_suffix` (`_resolve.py:486-506`): split
/// `external[+inv]:suffix` into its inventory name and `domain:name` suffix.
///
/// The `Err` case is Sphinx's `ValueError`, which
/// [`is_external_role`]-gated dispatch makes unreachable — index 8 of such a
/// name is always `+` or `:`. It is implemented and pinned anyway so the
/// invariant is checked rather than assumed.
pub fn inventory_and_name_suffix(name: &str) -> Result<(Option<&str>, &str), String> {
    let malformed = || format!("Malformed :external: role name: {name}");
    if !name.starts_with("external") || name.len() < 9 {
        return Err(malformed());
    }
    let suffix = &name[9..];
    match &name[8..9] {
        "+" => {
            let (inv_name, suffix) = suffix.split_once(':').unwrap_or((suffix, ""));
            Ok((Some(inv_name), suffix))
        }
        ":" => Ok((None, suffix)),
        _ => Err(malformed()),
    }
}

/// `_get_domain_role` (`_resolve.py:508-521`): no colon is a bare role name,
/// one colon splits domain from role, two or more is unusable.
pub fn domain_and_role(name: &str) -> (Option<&str>, Option<&str>) {
    let mut parts = name.split(':');
    let first = parts.next().unwrap_or_default();
    match (parts.next(), parts.next()) {
        (None, _) => (None, Some(first)),
        (Some(role), None) => (Some(first), Some(role)),
        _ => (None, None),
    }
}

/// What the parse layer should build for an `:external:...:` role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalRole {
    /// Emit a `pending_xref` for `domain:role`, stamped with `inventory`.
    Xref {
        inventory: Option<String>,
        domain: String,
        role: String,
    },
    /// Emit nothing, and report this once the build can locate it. Sphinx's
    /// role returns `([], [])` on every one of these
    /// (`_resolve.py:386-463`).
    Failed(Diagnostic),
}

/// The role-name half of `IntersphinxRole.run` (`_resolve.py:378-463`),
/// minus the inventory-existence check — that one needs the loaded
/// inventories, and is applied at resolution time by
/// [`external_inventory_missing`] so its warning still wins the race Sphinx
/// gives it (it is checked first).
pub fn external_role(name: &str) -> ExternalRole {
    let (inventory, suffix) = match inventory_and_name_suffix(name) {
        Ok(parsed) => parsed,
        Err(message) => return ExternalRole::Failed(Diagnostic::external(message)),
    };

    let (domain_name, role_name) = domain_and_role(suffix);
    let Some(role_name) = role_name else {
        return ExternalRole::Failed(Diagnostic::external(format!(
            "invalid external cross-reference suffix: {}",
            py_repr_str(suffix)
        )));
    };

    let inventory = inventory.map(str::to_string);
    if let Some(domain_name) = domain_name {
        // An explicit domain is the only one checked.
        let Some(spec) = domain(domain_name) else {
            return ExternalRole::Failed(Diagnostic::external(format!(
                "domain for external cross-reference not found: {}",
                py_repr_str(domain_name)
            )));
        };
        if !spec.has_role(role_name) {
            let base = format!(
                "role for external cross-reference not found in domain {}: {}",
                py_repr_str(domain_name),
                py_repr_str(role_name)
            );
            let message = match spec.roles_for_objtype(role_name).filter(|r| !r.is_empty()) {
                Some(roles) => format!(
                    "{base} (perhaps you meant one of: {})",
                    concat_strings(roles.iter().map(|role| (*role).to_string()))
                ),
                None => base,
            };
            return ExternalRole::Failed(Diagnostic::external(message));
        }
        return ExternalRole::Xref {
            inventory,
            domain: domain_name.to_string(),
            role: role_name.to_string(),
        };
    }

    // No domain given: try the default domain, then std.
    let candidates: Vec<&DomainSpec> = if DEFAULT_DOMAIN == "std" {
        vec![domain("std").expect("std is always registered")]
    } else {
        vec![
            domain(DEFAULT_DOMAIN).expect("the default domain is always registered"),
            domain("std").expect("std is always registered"),
        ]
    };
    for spec in &candidates {
        if spec.has_role(role_name) {
            return ExternalRole::Xref {
                inventory,
                domain: spec.name.to_string(),
                role: role_name.to_string(),
            };
        }
    }

    let domains_str = concat_strings(candidates.iter().map(|spec| spec.name.to_string()));
    let base = format!(
        "role for external cross-reference not found in domains {domains_str}: {}",
        py_repr_str(role_name)
    );
    let possible: BTreeSet<String> = candidates
        .iter()
        .filter_map(|spec| {
            spec.roles_for_objtype(role_name)
                .map(|roles| (spec.name, roles))
        })
        .flat_map(|(name, roles)| roles.iter().map(move |role| format!("{name}:{role}")))
        .collect();
    let message = if possible.is_empty() {
        base
    } else {
        format!(
            "{base} (perhaps you meant one of: {})",
            concat_strings(possible)
        )
    };
    ExternalRole::Failed(Diagnostic::external(message))
}

/// The inventory-existence check `IntersphinxRole.run` makes first
/// (`_resolve.py:385-390`), deferred to resolution time because that is
/// where this port knows what got loaded.
///
/// Returns the diagnostic when the named inventory is unknown and the
/// reference is not self-referential.
pub fn external_inventory_missing(isx: &Intersphinx, inventory: &str) -> Option<Diagnostic> {
    let self_referential = !isx.resolve_self.is_empty() && isx.resolve_self == inventory;
    if self_referential || isx.data.inventory_exists(inventory) {
        return None;
    }
    Some(Diagnostic::external(format!(
        "inventory for external cross-reference not found: {}",
        py_repr_str(inventory)
    )))
}

/// The failure `IntersphinxRoleResolver` reports for a stamped node nothing
/// matched (`_resolve.py:557-565`), logged with `type='ref'` and
/// `subtype=reftype`.
pub fn external_not_found(query: &XrefQuery<'_>) -> Diagnostic {
    Diagnostic {
        message: format!(
            "external {}:{} reference target not found: {}",
            query.refdomain, query.reftype, query.reftarget
        ),
        category: Some(format!("ref.{}", query.reftype)),
    }
}

/// `_concat_strings` (`_resolve.py:532-533`): sorted, `repr`'d, `', '`-joined.
fn concat_strings(strings: impl IntoIterator<Item = String>) -> String {
    let sorted: BTreeSet<String> = strings.into_iter().collect();
    sorted
        .iter()
        .map(|s| py_repr_str(s))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests;
