//! Serialized `BuildEnvironment` — the persistent build-state record that
//! mirrors Sphinx's `BuildEnvironment` (`environment/__init__.py`), scoped to
//! the fields this wave's read-and-resolve phase populates (see
//! `docs/superpowers/plans/2026-08-31-m2-wave4-research-spec-sphinx-env-toctree-domains.md`
//! §1 for the full attribute-by-attribute mapping this struct is drawn
//! from).
//!
//! Persisted as bincode (`bincode::serde` + `bincode::config::standard()`,
//! the same config [`crate::doctree::to_bincode`]/`from_bincode` use) to
//! `<cache_dir>/env.bin`. That file lives inside the cache directory
//! governed by the `.config-fingerprint` wipe protocol in `src/cache.rs`: a
//! change to a content-bearing configuration value nukes the whole cache
//! dir, `env.bin` included — which is the desired behavior, since the
//! environment was built under the old configuration. Note that this is
//! *broader* than Sphinx, which only invalidates its environment for config
//! values with rebuild class `'env'` and never deletes its doctrees; the
//! filter in `builder::EXCLUDED_FROM_FINGERPRINT` keeps at least the purely
//! operational flags (`-W`, `-n`) from triggering it.
//!
//! Every collection here is a `BTreeMap`/`BTreeSet` rather than the
//! `Hash*` equivalent so that bincode bytes and [`BuildEnvironment::snapshot`]
//! output are deterministic across runs and processes.

pub mod dependencies;
pub mod genindex;
pub mod metadata;
pub mod numbers;
pub mod resolve;
pub mod std_domain;
pub mod toctree;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map as JsonMap, Value as JsonValue};

use crate::doctree::Node;

// Seed data harvested from M1 create_standard_domains (deleted): the std/py
// object-type -> role tables, kept here verbatim for Task 8 to fold into the
// real `std`/`py` domain implementations that populate `StdDomainData`.
//
// py domain object types -> roles (lname == the type name itself):
//   module    -> [mod, obj]
//   function  -> [func, obj]
//   class     -> [class, obj]
//   method    -> [meth, obj]
//   attribute -> [attr, obj]
//   exception -> [exc, obj]
//   data      -> [data, obj]
//
// std domain object types -> roles:
//   doc       -> [doc]       (lname: "document")
//   label     -> [ref]       (lname: "label")
//   term      -> [term]      (lname: "term")
//   cmdoption -> [option]    (lname: "command line option")
//   envvar    -> [envvar]    (lname: "environment variable")

/// Bumped whenever the on-disk shape of [`BuildEnvironment`] changes.
/// [`BuildEnvironment::load`] discards (returns `None` for) any file whose
/// stored `version` doesn't match current — mirroring Sphinx's own
/// `ENV_VERSION` check, where a stale environment is simply rebuilt from
/// scratch rather than partially trusted.
pub const ENV_VERSION: u32 = 2;

/// The `env.bin` filename inside a build's cache directory.
const ENV_FILENAME: &str = "env.bin";

pub use std_domain::StdDomainData;

/// One entry harvested from a document's `index` nodes, as recorded in
/// Sphinx's `env.domaindata['index']['entries'][docname]`. `main` mirrors
/// Sphinx's literal `'main'`/`''` marker string as a bool; [`BuildEnvironment::snapshot`]
/// converts it back to that string form to match the oracle fixture shape.
///
/// Collected by [`genindex::process_doc`]; consumed by
/// [`genindex::create_index`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IndexEntryRecord {
    pub entry_type: String,
    pub value: String,
    pub target_id: String,
    pub main: bool,
    pub category_key: Option<String>,
}

/// What [`BuildEnvironment::get_outdated_files`] needs to know about the
/// filesystem, as the three questions it actually asks — so the computation
/// itself stays pure and the caller decides what "the source of `docname`"
/// and "the doctree of `docname`" mean.
///
/// Times are microseconds since the Unix epoch, the unit `all_docs` stores
/// (Sphinx's `_last_modified_time`). `None` means "cannot be stated" —
/// the file is gone, or unstat-able — which is Sphinx's `except OSError:
/// return True`: the document is outdated.
pub struct FileTimes<'a> {
    /// Modification time of the document's own source file.
    pub source_modified_us: &'a dyn Fn(&str) -> Option<u64>,
    /// Whether the document's persisted doctree is on disk. Sphinx stats
    /// `doctreedir/<docname>.doctree`; a doctree that exists but can no
    /// longer be *read* is caught later, when the read phase tries to load
    /// it, and re-read then.
    pub doctree_exists: &'a dyn Fn(&str) -> bool,
    /// Modification time of one of a document's `dependencies` entries.
    pub dependency_modified_us: &'a dyn Fn(&Path) -> Option<u64>,
}

/// The three sets `env.get_outdated_files` splits the project into
/// (`environment/__init__.py:521-554`).
///
/// `added` and `changed` are both read; they are kept apart because Sphinx
/// keeps them apart — the distinction drives the glob-toctree rule below
/// and the `%s added, %s changed, %s removed` progress line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outdated {
    /// Documents the environment has never seen.
    pub added: BTreeSet<String>,
    /// Documents whose recorded read is no longer good enough.
    pub changed: BTreeSet<String>,
    /// Documents the environment knows that the project no longer has.
    /// Each must be [`BuildEnvironment::clear_doc`]'d.
    pub removed: BTreeSet<String>,
}

impl Outdated {
    /// The documents this build has to read: `added | changed`.
    pub fn to_read(&self) -> BTreeSet<String> {
        self.added.union(&self.changed).cloned().collect()
    }
}

/// Persistent build-state record: the subset of Sphinx's `BuildEnvironment`
/// attributes this wave's read-and-resolve phase populates. See the module
/// doc comment for the persistence protocol.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BuildEnvironment {
    /// Format version of this serialized record. [`BuildEnvironment::save`]
    /// always stamps [`ENV_VERSION`] here; [`BuildEnvironment::load`]
    /// discards (returns `None` for) anything else.
    pub version: u32,
    /// Sphinx's `config.root_doc`: the document every whole-project walk of
    /// the toctree graph starts from ([`toctree::collect_relations`]) and
    /// the one document [`toctree::check_consistency`] never calls an
    /// orphan. Empty in a [`BuildEnvironment::default`]; the build stamps
    /// it from the configuration.
    pub root_doc: String,
    /// docname -> read time, in microseconds since the Unix epoch.
    pub all_docs: BTreeMap<String, u64>,
    /// docname -> absolute paths the document depends on (via `include`,
    /// literalinclude, etc.).
    pub dependencies: BTreeMap<String, BTreeSet<PathBuf>>,
    /// docname -> docnames it textually includes (docutils `include`).
    pub included: BTreeMap<String, BTreeSet<String>>,
    /// docnames that must always be re-read (e.g. they use `today`/`now`).
    pub reread_always: BTreeSet<String>,
    /// docname -> its bibliographic field list (`:orphan:`, `:tocdepth:`,
    /// ...), per [`metadata::document_metadata`].
    pub metadata: BTreeMap<String, BTreeMap<String, String>>,
    pub titles: BTreeMap<String, Node>,
    pub longtitles: BTreeMap<String, Node>,
    /// docname -> that document's local table of contents, doctree-shaped
    /// (a `bullet_list` node, mirroring Sphinx's `env.tocs`).
    pub tocs: BTreeMap<String, Node>,
    pub toc_num_entries: BTreeMap<String, u32>,
    /// docname -> (anchorname -> section-number tuple). `anchorname` is `''`
    /// for a document's own top entry, else `'#<id>'`.
    pub toc_secnumbers: BTreeMap<String, BTreeMap<String, Vec<u32>>>,
    /// docname -> (figtype -> (figure id -> figure-number tuple)).
    pub toc_fignumbers: BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<u32>>>>,
    /// docname -> docnames its toctree(s) directly include.
    pub toctree_includes: BTreeMap<String, Vec<String>>,
    /// included-docname -> docnames whose toctree includes it (the reverse
    /// of `toctree_includes`; used to know what to rebuild when a doc
    /// changes).
    pub files_to_rebuild: BTreeMap<String, BTreeSet<String>>,
    pub glob_toctrees: BTreeSet<String>,
    pub numbered_toctrees: BTreeSet<String>,
    pub std: StdDomainData,
    /// docname -> its `.. index::` entries, in document order.
    pub index_entries: BTreeMap<String, Vec<IndexEntryRecord>>,
}

impl BuildEnvironment {
    /// Load a previously saved environment from `<cache_dir>/env.bin`.
    ///
    /// Returns `None` if the file is missing, fails to decode, or was
    /// written by a different [`ENV_VERSION`] — in every case the caller's
    /// correct fallback is a fresh [`BuildEnvironment::default`], exactly
    /// like Sphinx discarding an incompatible `environment.pickle`.
    pub fn load(cache_dir: &Path) -> Option<Self> {
        let bytes = std::fs::read(cache_dir.join(ENV_FILENAME)).ok()?;
        let (env, _consumed): (Self, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).ok()?;
        if env.version != ENV_VERSION {
            return None;
        }
        Some(env)
    }

    /// Save this environment to `<cache_dir>/env.bin`, creating `cache_dir`
    /// if needed. Always stamps [`ENV_VERSION`] into the persisted bytes
    /// (regardless of `self.version`'s current in-memory value), so callers
    /// never need to remember to set it before saving.
    ///
    /// The in-memory `version` is only updated once the write has actually
    /// succeeded: a failed save must not leave the caller holding an
    /// environment that claims to have been written at the current version.
    pub fn save(&mut self, cache_dir: &Path) -> anyhow::Result<()> {
        let previous = std::mem::replace(&mut self.version, ENV_VERSION);
        let write = || -> anyhow::Result<()> {
            std::fs::create_dir_all(cache_dir)?;
            let bytes = bincode::serde::encode_to_vec(&*self, bincode::config::standard())?;
            std::fs::write(cache_dir.join(ENV_FILENAME), bytes)?;
            Ok(())
        };
        match write() {
            Ok(()) => Ok(()),
            Err(e) => {
                self.version = previous;
                Err(e)
            }
        }
    }

    /// Split `found` — the documents the project currently has — into what
    /// this build must read, and what it must forget.
    ///
    /// Port of `BuildEnvironment.get_outdated_files`
    /// (`environment/__init__.py:521-554`) together with the two steps
    /// `Builder.read` wraps around it (`builders/__init__.py:477-491`): a
    /// changed configuration re-reads everything, and adding or removing
    /// *any* file re-reads every document with a globbed toctree, whose
    /// entry list depends on which files exist rather than on its own text.
    ///
    /// Sphinx's `env-get-outdated` event — an extension's chance to add its
    /// own outdated documents — has no counterpart here; there are no
    /// extensions with read-phase state yet.
    pub fn get_outdated_files(
        &self,
        found: &BTreeSet<String>,
        config_changed: bool,
        times: &FileTimes<'_>,
    ) -> Outdated {
        let mut outdated = Outdated {
            removed: self
                .all_docs
                .keys()
                .filter(|docname| !found.contains(docname.as_str()))
                .cloned()
                .collect(),
            ..Default::default()
        };

        if config_changed {
            // Sphinx: `added = found_docs`, `changed` left empty — every
            // document is new as far as the old environment is concerned.
            outdated.added = found.clone();
            return outdated;
        }

        for docname in found {
            if !self.all_docs.contains_key(docname) {
                outdated.added.insert(docname.clone());
            } else if self.has_doc_changed(docname, times) {
                outdated.changed.insert(docname.clone());
            }
        }

        if !outdated.added.is_empty() || !outdated.removed.is_empty() {
            for docname in &self.glob_toctrees {
                if found.contains(docname) && !outdated.added.contains(docname) {
                    outdated.changed.insert(docname.clone());
                }
            }
        }

        outdated
    }

    /// `BuildEnvironment._has_doc_changed` (`environment/__init__.py:849-911`)
    /// for a document the environment already knows: the first of Sphinx's
    /// four reasons that holds wins.
    fn has_doc_changed(&self, docname: &str, times: &FileTimes<'_>) -> bool {
        if self.reread_always.contains(docname) {
            return true;
        }
        if !(times.doctree_exists)(docname) {
            return true;
        }
        let Some(&read_time) = self.all_docs.get(docname) else {
            return true;
        };
        match (times.source_modified_us)(docname) {
            None => return true,
            Some(modified) if modified > read_time => return true,
            Some(_) => {}
        }
        // Every dependency is compared against the time the *document* was
        // read, not against the document's own mtime: a file that changed
        // after the read invalidates it however old the document is.
        for dependency in self.dependencies.get(docname).into_iter().flatten() {
            match (times.dependency_modified_us)(dependency) {
                None => return true,
                Some(modified) if modified > read_time => return true,
                Some(_) => {}
            }
        }
        false
    }

    /// Remove every trace of `docname` from the environment — the Rust
    /// mirror of Sphinx's `BuildEnvironment.clear_doc` *plus* every
    /// `EnvironmentCollector.clear_doc`/`Domain.clear_doc` that fires
    /// alongside it via the `env-purge-doc` event (Sphinx dispatches these
    /// separately; here they're one method since there's no event bus).
    /// See `environment/__init__.py:412` (base), `environment/collectors/
    /// toctree.py:30` (toctree fields + `files_to_rebuild`),
    /// `environment/collectors/title.py:23` (titles/longtitles),
    /// `environment/collectors/dependencies.py:24` (dependencies),
    /// `environment/collectors/metadata.py:22` (metadata),
    /// `domains/std/__init__.py:896` (std domain), `domains/index.py:41`
    /// (index entries).
    pub fn clear_doc(&mut self, docname: &str) {
        self.all_docs.remove(docname);
        self.included.remove(docname);
        self.reread_always.remove(docname);
        self.dependencies.remove(docname);
        self.metadata.remove(docname);

        self.titles.remove(docname);
        self.longtitles.remove(docname);

        self.tocs.remove(docname);
        self.toc_secnumbers.remove(docname);
        self.toc_fignumbers.remove(docname);
        self.toc_num_entries.remove(docname);
        self.toctree_includes.remove(docname);
        self.glob_toctrees.remove(docname);
        self.numbered_toctrees.remove(docname);

        // Sphinx: `for subfn, fnset in list(files_to_rebuild.items()):
        // fnset.discard(docname); if not fnset: del files_to_rebuild[subfn]`.
        self.files_to_rebuild.retain(|_, containing| {
            containing.remove(docname);
            !containing.is_empty()
        });

        self.std
            .progoptions
            .retain(|_, (fn_, _)| fn_.as_str() != docname);
        self.std
            .objects
            .retain(|_, (fn_, _)| fn_.as_str() != docname);
        self.std.terms.retain(|_, (fn_, _)| fn_.as_str() != docname);
        self.std
            .labels
            .retain(|_, (fn_, _, _)| fn_.as_str() != docname);
        self.std
            .anonlabels
            .retain(|_, (fn_, _)| fn_.as_str() != docname);

        self.index_entries.remove(docname);
    }

    /// A deterministic JSON view of this environment, shaped to line up
    /// with the `env_differential` oracle fixture (`tests/env_differential.rs`,
    /// `tests/fixtures/env_differential.json`) so later tasks can diff
    /// straight against it. `std.objects`/`std.progoptions` use tuple keys,
    /// which `serde_json` cannot serialize as map keys directly, so those
    /// (and `index_entries`, whose `main: bool` must become the oracle's
    /// literal `"main"`/`""` string) are hand-converted into the fixture's
    /// list/tuple shapes rather than derived via a blanket `to_value(self)`.
    pub fn snapshot(&self) -> JsonValue {
        let objects: Vec<JsonValue> = self
            .std
            .objects
            .iter()
            .map(|((objtype, name), (docname, labelid))| {
                json!({
                    "objtype": objtype,
                    "name": name,
                    "docname": docname,
                    "labelid": labelid,
                })
            })
            .collect();

        let progoptions: Vec<JsonValue> = self
            .std
            .progoptions
            .iter()
            .map(|((program, name), (docname, labelid))| {
                json!({
                    "program": program,
                    "name": name,
                    "docname": docname,
                    "labelid": labelid,
                })
            })
            .collect();

        let mut index_entries = JsonMap::new();
        for (docname, entries) in &self.index_entries {
            let arr: Vec<JsonValue> = entries
                .iter()
                .map(|e| {
                    json!([
                        e.entry_type,
                        e.value,
                        e.target_id,
                        if e.main { "main" } else { "" },
                        e.category_key,
                    ])
                })
                .collect();
            index_entries.insert(docname.clone(), JsonValue::Array(arr));
        }

        // `relations` is derived, not stored — exactly like Sphinx's
        // `collect_relations()`, which recomputes it from the toctree graph
        // on demand.
        let relations: JsonMap<String, JsonValue> = toctree::collect_relations(self)
            .into_iter()
            .map(|(docname, (parent, prev, next))| (docname, json!([parent, prev, next])))
            .collect();

        json!({
            "version": self.version,
            "root_doc": self.root_doc,
            "all_docs": self.all_docs,
            "relations": JsonValue::Object(relations),
            "metadata": self.metadata,
            "dependencies": self.dependencies,
            "included": self.included,
            "reread_always": self.reread_always,
            "titles_pformat": pformat_map(&self.titles),
            "longtitles_pformat": pformat_map(&self.longtitles),
            "tocs_pformat": pformat_map(&self.tocs),
            "toc_num_entries": self.toc_num_entries,
            "toc_secnumbers": self.toc_secnumbers,
            "toc_fignumbers": self.toc_fignumbers,
            "toctree_includes": self.toctree_includes,
            "files_to_rebuild": self.files_to_rebuild,
            "glob_toctrees": self.glob_toctrees,
            "numbered_toctrees": self.numbered_toctrees,
            "std": {
                "labels": self.std.labels,
                "anonlabels": self.std.anonlabels,
                "objects": objects,
                "progoptions": progoptions,
                "terms": self.std.terms,
            },
            "index_entries": JsonValue::Object(index_entries),
        })
    }
}

/// docname -> pseudo-XML pformat of a doctree-shaped node, matching the
/// oracle fixture's `tocs_pformat` string shape.
fn pformat_map(nodes: &BTreeMap<String, Node>) -> JsonValue {
    let map: JsonMap<String, JsonValue> = nodes
        .iter()
        .map(|(docname, node)| (docname.clone(), JsonValue::String(node.pformat())))
        .collect();
    JsonValue::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctree::{kinds, Span};

    fn sample_node() -> Node {
        let mut root = Node::elem(kinds::BULLET_LIST, Span::ZERO);
        let mut item = Node::elem(kinds::LIST_ITEM, Span::ZERO);
        item.children
            .push(Node::text_node("Chapter One", Span::ZERO));
        root.children.push(item);
        root
    }

    fn populated_env() -> BuildEnvironment {
        let mut env = BuildEnvironment {
            version: ENV_VERSION,
            root_doc: "index".to_string(),
            ..Default::default()
        };
        env.all_docs.insert("index".to_string(), 1_700_000_000);
        env.metadata.insert(
            "index".to_string(),
            BTreeMap::from([("orphan".to_string(), String::new())]),
        );
        env.dependencies.insert(
            "index".to_string(),
            BTreeSet::from([PathBuf::from("/src/index.rst")]),
        );
        env.included.insert(
            "index".to_string(),
            BTreeSet::from(["chapters/intro".to_string()]),
        );
        env.reread_always.insert("index".to_string());
        env.titles.insert("index".to_string(), sample_node());
        env.longtitles.insert("index".to_string(), sample_node());
        env.tocs.insert("index".to_string(), sample_node());
        env.toc_num_entries.insert("index".to_string(), 3);
        env.toc_secnumbers.insert(
            "index".to_string(),
            BTreeMap::from([(String::new(), vec![1]), ("#sec".to_string(), vec![1, 1])]),
        );
        env.toc_fignumbers.insert(
            "index".to_string(),
            BTreeMap::from([(
                "figure".to_string(),
                BTreeMap::from([("fig1".to_string(), vec![1])]),
            )]),
        );
        env.toctree_includes.insert(
            "index".to_string(),
            vec!["chapters/intro".to_string(), "chapters/two".to_string()],
        );
        env.files_to_rebuild.insert(
            "chapters/intro".to_string(),
            BTreeSet::from(["index".to_string()]),
        );
        env.glob_toctrees.insert("index".to_string());
        env.numbered_toctrees.insert("index".to_string());
        // std/index entries "owned" by the "index" doc itself (docname is
        // the value's first component) -- e.g. "index.rst" contains
        // `.. envvar:: PATH` directly.
        env.std.labels.insert(
            "intro".to_string(),
            (
                "index".to_string(),
                "intro-id".to_string(),
                "Introduction".to_string(),
            ),
        );
        env.std.anonlabels.insert(
            "intro".to_string(),
            ("index".to_string(), "intro-id".to_string()),
        );
        env.std.objects.insert(
            ("envvar".to_string(), "PATH".to_string()),
            ("index".to_string(), "envvar-path".to_string()),
        );
        env.std.progoptions.insert(
            (Some("myprog".to_string()), "--verbose".to_string()),
            ("index".to_string(), "cmdoption-verbose".to_string()),
        );
        env.std.terms.insert(
            "glossary term".to_string(),
            ("index".to_string(), "term-glossary-term".to_string()),
        );
        env.index_entries.insert(
            "index".to_string(),
            vec![IndexEntryRecord {
                entry_type: "single".to_string(),
                value: "PATH".to_string(),
                target_id: "index-0".to_string(),
                main: true,
                category_key: None,
            }],
        );
        env
    }

    #[test]
    fn round_trip_through_bincode_preserves_node_valued_fields() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut env = populated_env();

        env.save(tmp.path()).expect("save succeeds");
        let restored = BuildEnvironment::load(tmp.path()).expect("load succeeds");

        assert_eq!(restored, env);
        // Node-valued fields specifically: bincode round-trips them exactly,
        // not just "some value under the same key".
        assert_eq!(restored.titles["index"], sample_node());
        assert_eq!(restored.tocs["index"].pformat(), sample_node().pformat());
    }

    #[test]
    fn save_always_stamps_current_env_version() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut env = BuildEnvironment {
            version: 0, // stale/uninitialized in-memory value
            ..Default::default()
        };

        env.save(tmp.path()).unwrap();

        assert_eq!(env.version, ENV_VERSION);
        let restored = BuildEnvironment::load(tmp.path()).unwrap();
        assert_eq!(restored.version, ENV_VERSION);
    }

    #[test]
    fn failed_save_leaves_the_in_memory_version_untouched() {
        // A file where the cache dir should be: create_dir_all fails, so the
        // environment must not be left claiming it was saved at the current
        // version.
        let tmp = tempfile::TempDir::new().unwrap();
        let blocked = tmp.path().join("not-a-dir");
        std::fs::write(&blocked, b"").unwrap();

        let mut env = BuildEnvironment {
            version: 0,
            ..Default::default()
        };
        assert!(env.save(&blocked).is_err());
        assert_eq!(env.version, 0);
    }

    #[test]
    fn load_returns_none_when_file_is_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(BuildEnvironment::load(tmp.path()).is_none());
    }

    #[test]
    fn load_returns_none_on_decode_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(ENV_FILENAME),
            b"not a valid bincode blob at all",
        )
        .unwrap();
        assert!(BuildEnvironment::load(tmp.path()).is_none());
    }

    #[test]
    fn load_returns_none_when_version_does_not_match_current() {
        let tmp = tempfile::TempDir::new().unwrap();
        let stale = BuildEnvironment {
            version: ENV_VERSION + 1,
            ..Default::default()
        };
        let bytes = bincode::serde::encode_to_vec(&stale, bincode::config::standard()).unwrap();
        std::fs::write(tmp.path().join(ENV_FILENAME), bytes).unwrap();

        assert!(BuildEnvironment::load(tmp.path()).is_none());
    }

    #[test]
    fn clear_doc_scrubs_every_per_doc_field() {
        let mut env = populated_env();
        // A second doc ("other") also has "chapters/intro" in its toctree,
        // to prove the files_to_rebuild key survives clear_doc("index")
        // because the value-set isn't left empty.
        env.files_to_rebuild
            .get_mut("chapters/intro")
            .unwrap()
            .insert("other".to_string());
        env.all_docs.insert("other".to_string(), 1_700_000_001);

        env.clear_doc("index");

        assert!(!env.all_docs.contains_key("index"));
        assert!(!env.included.contains_key("index"));
        assert!(!env.reread_always.contains("index"));
        assert!(!env.dependencies.contains_key("index"));
        assert!(!env.metadata.contains_key("index"));
        assert!(!env.titles.contains_key("index"));
        assert!(!env.longtitles.contains_key("index"));
        assert!(!env.tocs.contains_key("index"));
        assert!(!env.toc_secnumbers.contains_key("index"));
        assert!(!env.toc_fignumbers.contains_key("index"));
        assert!(!env.toc_num_entries.contains_key("index"));
        assert!(!env.toctree_includes.contains_key("index"));
        assert!(!env.glob_toctrees.contains("index"));
        assert!(!env.numbered_toctrees.contains("index"));

        // files_to_rebuild: "index" removed from the value-set, key
        // survives because "other" still references it.
        assert_eq!(
            env.files_to_rebuild.get("chapters/intro"),
            Some(&BTreeSet::from(["other".to_string()]))
        );

        // std/index domain entries are keyed by label/term/object name, not
        // docname; clear_doc scrubs them by matching the docname *inside*
        // each entry's value, which is "index" here (an envvar/label/term
        // defined directly in index.rst).
        // The preseeded virtual labels (genindex/modindex/py-modindex/
        // search) belong to no source document, so they survive.
        assert!(!env.std.labels.contains_key("intro"));
        assert!(!env.std.anonlabels.contains_key("intro"));
        assert_eq!(env.std.labels, StdDomainData::default().labels);
        assert_eq!(env.std.anonlabels, StdDomainData::default().anonlabels);
        assert!(env.std.objects.is_empty());
        assert!(env.std.progoptions.is_empty());
        assert!(env.std.terms.is_empty());
        assert!(env.index_entries.is_empty());
    }

    #[test]
    fn clear_doc_deletes_files_to_rebuild_key_when_value_set_becomes_empty() {
        let mut env = BuildEnvironment::default();
        env.files_to_rebuild.insert(
            "chapters/intro".to_string(),
            BTreeSet::from(["index".to_string()]),
        );

        env.clear_doc("index");

        assert!(
            !env.files_to_rebuild.contains_key("chapters/intro"),
            "an emptied value-set must delete its key, not linger as an empty set"
        );
    }

    /// A synthetic filesystem for the outdated computation: every document
    /// has a doctree and a source read one second before its recorded read
    /// time, and no dependency exists unless the test adds one.
    #[derive(Default)]
    struct Fs {
        sources: BTreeMap<String, Option<u64>>,
        doctrees: BTreeSet<String>,
        deps: BTreeMap<PathBuf, Option<u64>>,
    }

    const READ_TIME: u64 = 1_000_000;

    /// Two documents, both read at [`READ_TIME`], both up to date.
    fn steady_state() -> (BuildEnvironment, Fs) {
        let mut env = BuildEnvironment {
            root_doc: "index".to_string(),
            ..Default::default()
        };
        env.all_docs.insert("index".to_string(), READ_TIME);
        env.all_docs.insert("a".to_string(), READ_TIME);
        let fs = Fs {
            sources: BTreeMap::from([
                ("index".to_string(), Some(READ_TIME - 1)),
                ("a".to_string(), Some(READ_TIME - 1)),
            ]),
            doctrees: BTreeSet::from(["index".to_string(), "a".to_string()]),
            deps: BTreeMap::new(),
        };
        (env, fs)
    }

    fn found(docnames: &[&str]) -> BTreeSet<String> {
        docnames.iter().map(|d| d.to_string()).collect()
    }

    fn outdated_with(
        env: &BuildEnvironment,
        fs: &Fs,
        docnames: &[&str],
        config_changed: bool,
    ) -> Outdated {
        env.get_outdated_files(
            &found(docnames),
            config_changed,
            &FileTimes {
                source_modified_us: &|docname| fs.sources.get(docname).copied().flatten(),
                doctree_exists: &|docname| fs.doctrees.contains(docname),
                dependency_modified_us: &|path| fs.deps.get(path).copied().flatten(),
            },
        )
    }

    fn outdated(env: &BuildEnvironment, fs: &Fs, docnames: &[&str]) -> Outdated {
        outdated_with(env, fs, docnames, false)
    }

    #[test]
    fn nothing_is_outdated_in_a_steady_state() {
        let (env, fs) = steady_state();
        let out = outdated(&env, &fs, &["index", "a"]);
        assert_eq!(out, Outdated::default());
        assert!(out.to_read().is_empty());
    }

    #[test]
    fn a_document_the_environment_has_never_seen_is_added() {
        let (env, mut fs) = steady_state();
        fs.sources.insert("new".to_string(), Some(READ_TIME));
        let out = outdated(&env, &fs, &["index", "a", "new"]);
        assert_eq!(out.added, found(&["new"]));
        assert!(out.changed.is_empty());
        assert!(out.removed.is_empty());
    }

    #[test]
    fn a_document_that_is_gone_is_removed() {
        let (env, fs) = steady_state();
        let out = outdated(&env, &fs, &["index"]);
        assert_eq!(out.removed, found(&["a"]));
        assert!(out.added.is_empty());
        // The still-present document is *not* dragged along by its
        // neighbour's disappearance (only glob toctrees are).
        assert!(out.changed.is_empty());
    }

    #[test]
    fn a_changed_configuration_re_reads_everything() {
        let (env, fs) = steady_state();
        let out = outdated_with(&env, &fs, &["index", "a"], true);
        assert_eq!(out.added, found(&["index", "a"]));
        assert!(
            out.changed.is_empty(),
            "sphinx puts every document in `added` and leaves `changed` empty"
        );
    }

    #[test]
    fn a_source_newer_than_its_read_time_has_changed() {
        let (env, mut fs) = steady_state();
        fs.sources.insert("a".to_string(), Some(READ_TIME + 1));
        assert_eq!(outdated(&env, &fs, &["index", "a"]).changed, found(&["a"]));
    }

    #[test]
    fn a_source_read_within_the_same_microsecond_has_not_changed() {
        let (env, mut fs) = steady_state();
        fs.sources.insert("a".to_string(), Some(READ_TIME));
        assert!(
            outdated(&env, &fs, &["index", "a"]).changed.is_empty(),
            "the comparison is strictly-newer, like sphinx's"
        );
    }

    #[test]
    fn an_unstattable_source_has_changed() {
        let (env, mut fs) = steady_state();
        fs.sources.insert("a".to_string(), None);
        assert_eq!(outdated(&env, &fs, &["index", "a"]).changed, found(&["a"]));
    }

    #[test]
    fn a_missing_doctree_file_has_changed() {
        let (env, mut fs) = steady_state();
        fs.doctrees.remove("a");
        assert_eq!(outdated(&env, &fs, &["index", "a"]).changed, found(&["a"]));
    }

    #[test]
    fn a_document_that_asked_to_be_re_read_always_has_changed() {
        let (mut env, fs) = steady_state();
        env.reread_always.insert("a".to_string());
        assert_eq!(outdated(&env, &fs, &["index", "a"]).changed, found(&["a"]));
    }

    #[test]
    fn a_dependency_newer_than_the_read_time_has_changed() {
        let (mut env, mut fs) = steady_state();
        let pic = PathBuf::from("/src/pic.png");
        env.dependencies
            .insert("a".to_string(), BTreeSet::from([pic.clone()]));

        fs.deps.insert(pic.clone(), Some(READ_TIME - 1));
        assert!(outdated(&env, &fs, &["index", "a"]).changed.is_empty());

        // Note the comparison: the dependency's mtime against the time the
        // *document* was read, not against the document's own mtime.
        fs.deps.insert(pic, Some(READ_TIME + 1));
        assert_eq!(outdated(&env, &fs, &["index", "a"]).changed, found(&["a"]));
    }

    #[test]
    fn a_missing_dependency_has_changed() {
        let (mut env, mut fs) = steady_state();
        let pic = PathBuf::from("/src/pic.png");
        env.dependencies
            .insert("a".to_string(), BTreeSet::from([pic.clone()]));
        fs.deps.insert(pic, None);
        assert_eq!(outdated(&env, &fs, &["index", "a"]).changed, found(&["a"]));
    }

    #[test]
    fn adding_or_removing_a_file_re_reads_every_glob_toctree() {
        let (mut env, mut fs) = steady_state();
        env.glob_toctrees.insert("index".to_string());
        // A glob container that is no longer part of the project is not
        // resurrected by the re-read.
        env.glob_toctrees.insert("gone".to_string());

        // Nothing added or removed: the container is left alone.
        assert!(outdated(&env, &fs, &["index", "a"]).changed.is_empty());

        fs.sources.insert("new".to_string(), Some(READ_TIME));
        fs.doctrees.insert("new".to_string());
        let added = outdated(&env, &fs, &["index", "a", "new"]);
        assert_eq!(added.added, found(&["new"]));
        assert_eq!(added.changed, found(&["index"]));

        let removed = outdated(&env, &fs, &["index"]);
        assert_eq!(removed.removed, found(&["a"]));
        assert_eq!(removed.changed, found(&["index"]));
    }

    #[test]
    fn a_glob_container_that_is_new_itself_stays_in_added() {
        let (mut env, mut fs) = steady_state();
        env.glob_toctrees.insert("new".to_string());
        fs.sources.insert("new".to_string(), Some(READ_TIME));
        let out = outdated(&env, &fs, &["index", "a", "new"]);
        assert_eq!(out.added, found(&["new"]));
        assert!(
            out.changed.is_empty(),
            "a document is read once; being added already covers it"
        );
    }

    #[test]
    fn the_read_set_is_the_added_and_changed_documents() {
        let (mut env, mut fs) = steady_state();
        fs.sources.insert("new".to_string(), Some(READ_TIME));
        fs.doctrees.remove("a");
        env.all_docs.insert("gone".to_string(), READ_TIME);

        let out = outdated(&env, &fs, &["index", "a", "new"]);
        assert_eq!(out.to_read(), found(&["a", "new"]));
        assert_eq!(out.removed, found(&["gone"]));
    }

    #[test]
    fn snapshot_converts_tuple_keyed_maps_and_index_entry_main_flag() {
        let env = populated_env();
        let snapshot = env.snapshot();

        let objects = snapshot["std"]["objects"].as_array().unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0]["objtype"], "envvar");
        assert_eq!(objects[0]["name"], "PATH");
        assert_eq!(objects[0]["docname"], "index");

        let progoptions = snapshot["std"]["progoptions"].as_array().unwrap();
        assert_eq!(progoptions[0]["program"], "myprog");
        assert_eq!(progoptions[0]["name"], "--verbose");

        let entries = snapshot["index_entries"]["index"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        let entry = entries[0].as_array().unwrap();
        assert_eq!(entry[0], "single");
        assert_eq!(entry[3], "main"); // bool true -> literal "main"

        assert_eq!(
            snapshot["tocs_pformat"]["index"],
            JsonValue::String(sample_node().pformat())
        );
    }
}
