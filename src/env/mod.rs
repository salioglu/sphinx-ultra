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
//! configuration change nukes the whole cache dir, `env.bin` included —
//! which is exactly the desired behavior, since the environment was built
//! under the old configuration.
//!
//! Every collection here is a `BTreeMap`/`BTreeSet` rather than the
//! `Hash*` equivalent so that bincode bytes and [`BuildEnvironment::snapshot`]
//! output are deterministic across runs and processes.

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
