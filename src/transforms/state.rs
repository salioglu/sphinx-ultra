//! The docutils document state a parse leaves behind (wave 5a, decision 1;
//! [DS §8.1, §12.2]).
//!
//! Sphinx parses into one docutils `document` and then runs every read
//! transform over that same object, so the transforms read more than the
//! tree: every id the parse registered, those of loose messages and of
//! dropped nodes included, the auto-id counter, and the name table in
//! registration order. [`DocumentState`] is that object state. The parser
//! threads one through every nested and detached sub-parse and hands it on
//! as `ParseOutput.state`.

use std::collections::{HashMap, HashSet};

use crate::doctree::ids::{make_id, DupnameFixup};
use crate::doctree::messages::{INFO, WARNING};
use crate::doctree::{kinds, AttrValue, Node};
use crate::rst::block::IncludeClip;
use crate::rst::{
    DirectiveRecord, ObjectRegistration, ParseLogWarning, ProgramOptionRecord, PyModuleRecord,
    PyObjectRecord, RoleRecord, ToctreeRecord,
};

/// The phase of the events the parse records ([`IdEvent::phase`]).
const PARSE_PHASE: &str = "parse";

/// One docutils `document.set_id` call on a node without ids, as the
/// read-state fixture's `set_id` wrapper records it for its `id_trace`
/// (`probe_registries.py`'s rule; [DS §1, §11]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdEvent {
    /// `"parse"`, or the dotted Python name of the transform that was
    /// running ([`IdAllocator::set_phase`]).
    pub phase: String,
    /// The node's kind (its docutils tagname).
    pub kind: &'static str,
    pub id: String,
    /// Whether the node the id names is still in the final tree. The
    /// allocator records `false`; the read sets it once the tree is final.
    pub attached: bool,
}

/// docutils' id bookkeeping on `document` under Sphinx's settings
/// (`id_prefix = ''`, `auto_id_prefix = 'id'`,
/// `SP/sphinx/environment/__init__.py:60`): the `ids` key set and
/// `id_counter['id']` ([DS §5.1, §5.7]).
#[derive(Debug, Default, Clone)]
pub struct IdAllocator {
    /// `document.ids` keys: every id the document registered. Those of
    /// loose messages and of nodes the parse dropped are kept, because they
    /// still block reuse.
    taken: HashSet<String>,
    /// `document.id_counter['id']`, the one auto-id counter.
    counter: u32,
    /// One event per [`Self::allocate`] while enabled
    /// ([`Self::enable_trace`]).
    trace: Option<Vec<IdEvent>>,
    /// The phase new events record; `None` is the parse.
    phase: Option<String>,
}

impl IdAllocator {
    /// docutils `document.set_id` for a node without ids
    /// (`SP/docutils/nodes.py:1844-1875`): the first of the node's `names`
    /// whose `make_id` is non-empty and free, else the next free `id<N>`
    /// from the shared counter. Under `auto_id_prefix = 'id'` the `'%'`
    /// branches are dead, so docutils' `suggested_prefix` never applies and
    /// is not taken ([DS §5.6]). The id is registered; with the trace on,
    /// one [`IdEvent`] records it with the node's `kind`.
    pub fn allocate(&mut self, kind: &'static str, names: &[String]) -> String {
        let from_name = names
            .iter()
            .map(|name| make_id(name))
            .find(|base| !base.is_empty() && !self.taken.contains(base));
        let id = match from_name {
            Some(id) => id,
            None => loop {
                self.counter += 1;
                let id = format!("id{}", self.counter);
                if !self.taken.contains(&id) {
                    break id;
                }
            },
        };
        self.taken.insert(id.clone());
        if let Some(trace) = &mut self.trace {
            trace.push(IdEvent {
                phase: self.phase.as_deref().unwrap_or(PARSE_PHASE).to_string(),
                kind,
                id: id.clone(),
                attached: false,
            });
        }
        id
    }

    /// docutils `document.set_id` for a node that already carries ids
    /// (`nodes.py:1832-1843`), one id at a time: `ids.setdefault(id, node)`.
    /// `false` means the id was registered before, docutils' `Duplicate ID`
    /// case when another node holds it. Never traced: the fixture's wrapper
    /// records only nodes without ids.
    pub fn register(&mut self, id: &str) -> bool {
        !self.taken.contains(id) && self.taken.insert(id.to_string())
    }

    /// `id in document.ids`.
    pub fn is_taken(&self, id: &str) -> bool {
        self.taken.contains(id)
    }

    /// Record one [`IdEvent`] per [`Self::allocate`] from now on (tests
    /// only, `ParseOptions::trace_ids`).
    pub fn enable_trace(&mut self) {
        self.trace.get_or_insert_with(Vec::new);
    }

    /// The phase later events record: the dotted Python name of the
    /// transform about to run.
    pub fn set_phase(&mut self, phase: String) {
        self.phase = Some(phase);
    }

    /// The recorded events in allocation order; empty unless enabled.
    pub fn trace(&self) -> &[IdEvent] {
        self.trace.as_deref().unwrap_or_default()
    }

    /// The recorded events, for the read to fill in [`IdEvent::attached`].
    pub fn trace_mut(&mut self) -> &mut [IdEvent] {
        self.trace.as_deref_mut().unwrap_or_default()
    }
}

/// One name's `document.nameids` and `nametypes` entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameEntry {
    /// `nameids[name]`: the id the name maps to; `None` once duplicates
    /// cleared it.
    pub id: Option<String>,
    /// `nametypes[name]`: whether an explicit target holds the name.
    pub explicit: bool,
    /// The `refuri` and `refname` of the node [`Self::id`] names, as it was
    /// bound. `set_duplicate_name_id` compares a new node's with them
    /// (`nodes.py:1943-1951`), and the parser, not the document, owns that
    /// node.
    pub refuri: Option<String>,
    pub refname: Option<String>,
}

impl NameEntry {
    /// The name maps to `id`, the id of the node `view` shows.
    fn bound_to(view: NodeView<'_>, id: &str, explicit: bool) -> NameEntry {
        NameEntry {
            id: Some(id.to_string()),
            explicit,
            refuri: view.refuri.map(str::to_string),
            refname: view.refname.map(str::to_string),
        }
    }

    /// `nameids[name] = None`: no node holds the name.
    fn clear(&mut self) {
        self.id = None;
        self.refuri = None;
        self.refname = None;
    }
}

/// What binding a node's names reads from the node. docutils'
/// `set_name_id_map` and `set_duplicate_name_id` read the live node; the
/// parser builds the view from the node it holds ([`NodeView::of`]).
#[derive(Debug, Clone, Copy)]
pub struct NodeView<'a> {
    pub kind: &'static str,
    /// The names to bind, in order: docutils iterates a snapshot,
    /// `tuple(node['names'])`.
    pub names: &'a [String],
    pub refuri: Option<&'a str>,
    pub refname: Option<&'a str>,
}

impl<'a> NodeView<'a> {
    /// The view of a node the parser holds: its kind, its names and its
    /// `refuri` and `refname` attributes.
    pub fn of(node: &'a Node) -> NodeView<'a> {
        let attr = |key: &'static str| match node.get(key) {
            Some(AttrValue::Str(value)) => Some(value.as_str()),
            _ => None,
        };
        NodeView {
            kind: node.kind,
            names: &node.attrs.names,
            refuri: attr("refuri"),
            refname: attr("refname"),
        }
    }
}

/// One duplicate name [`NameTable::bind`] reports: what docutils'
/// `set_duplicate_name_id` (`nodes.py:1929-1990`) does to the nodes and
/// the message it raises, for the caller to apply where it holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DupOutcome {
    pub name: String,
    /// The message's reporter level: 1 (INFO) or 2 (WARNING).
    pub level: u8,
    /// The message text.
    pub message: String,
    /// Whether the message's `backrefs` hold the bound node's id: always,
    /// except for a `target` with a `refuri` ("don't add backref id for
    /// empty targets").
    pub backrefs: bool,
    /// docutils `dupname(node, name)` on the node being bound.
    pub dupname_new: bool,
    /// docutils `dupname(old_node, name)`: the id naming the earlier node.
    pub dupname_old: Option<String>,
}

/// docutils `document.nameids` and `nametypes` (same keys), in insertion
/// order: `StandardDomain.process_doc` iterates `nametypes`
/// (`SP/sphinx/domains/std/__init__.py:940`), so the order is state too
/// ([DS §0, S2]).
#[derive(Debug, Default, Clone)]
pub struct NameTable {
    entries: Vec<(String, NameEntry)>,
    index: HashMap<String, usize>,
}

impl NameTable {
    pub fn get(&self, name: &str) -> Option<&NameEntry> {
        self.index.get(name).map(|&slot| &self.entries[slot].1)
    }

    /// docutils `document.has_name`.
    pub fn has_name(&self, name: &str) -> bool {
        self.index.contains_key(name)
    }

    /// Every name and its entry, in the order the names were first bound.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &NameEntry)> {
        self.entries
            .iter()
            .map(|(name, entry)| (name.as_str(), entry))
    }

    /// docutils `set_name_id_map(node, id, msgnode, explicit)`
    /// (`nodes.py:1877-1927`): binds each of the node's names to `id`, the
    /// id `set_id` gave it. A name already in the table goes through
    /// `set_duplicate_name_id`, including its first branch: a new node
    /// referring to the same `refname` or `refuri` as the bound one leaves
    /// that one bound. Returns the duplicates reported, in name order.
    pub fn bind(&mut self, node: NodeView<'_>, id: &str, explicit: bool) -> Vec<DupOutcome> {
        let mut reported = Vec::new();
        for name in node.names {
            match self.index.get(name.as_str()) {
                Some(&slot) => {
                    let entry = &mut self.entries[slot].1;
                    reported.extend(set_duplicate_name_id(entry, name, node, id, explicit));
                }
                None => {
                    self.index.insert(name.clone(), self.entries.len());
                    self.entries
                        .push((name.clone(), NameEntry::bound_to(node, id, explicit)));
                }
            }
        }
        reported
    }
}

/// docutils `set_duplicate_name_id` (`nodes.py:1929-1990`) for `name`,
/// whose binding is `entry`. `None` when docutils reports nothing (a new
/// explicit target taking a name only implicit duplicates had cleared).
fn set_duplicate_name_id(
    entry: &mut NameEntry,
    name: &str,
    node: NodeView<'_>,
    id: &str,
    explicit: bool,
) -> Option<DupOutcome> {
    let old_id = entry.id.clone();
    let old_explicit = entry.explicit;
    entry.explicit = old_explicit || explicit;
    let backrefs = !(node.kind == kinds::TARGET && node.refuri.is_some());
    let report = |level, message: String, dupname_new, dupname_old| DupOutcome {
        name: name.to_string(),
        level,
        message,
        backrefs,
        dupname_new,
        dupname_old,
    };
    let same_reference = node
        .refname
        .is_some_and(|refname| entry.refname.as_deref() == Some(refname))
        || node
            .refuri
            .is_some_and(|refuri| entry.refuri.as_deref() == Some(refuri));
    if old_id.is_some() && same_reference {
        // "indirect targets with same reference -> keep old target";
        // `ref = node.get('refuri') or node.get('refname')`.
        let reference = node
            .refuri
            .filter(|refuri| !refuri.is_empty())
            .or(node.refname)
            .unwrap_or("None");
        return Some(report(
            INFO,
            format!("Duplicate name \"{name}\" for external target \"{reference}\"."),
            true,
            None,
        ));
    }
    if explicit {
        if old_explicit {
            if old_id.is_some() {
                entry.clear();
            }
            Some(report(
                WARNING,
                format!("Duplicate explicit target name: \"{name}\"."),
                true,
                old_id,
            ))
        } else {
            // New explicit, old implicit: the new target takes the name.
            *entry = NameEntry::bound_to(node, id, true);
            old_id.map(|old_id| {
                report(
                    INFO,
                    format!("Target name overrides implicit target name \"{name}\"."),
                    false,
                    Some(old_id),
                )
            })
        }
    } else {
        let dupname_old = if old_explicit { None } else { old_id };
        if dupname_old.is_some() {
            entry.clear();
        }
        Some(report(
            INFO,
            format!("Duplicate implicit target name: \"{name}\"."),
            true,
            dupname_old,
        ))
    }
}

/// An insertion-ordered multimap: docutils' dict-of-lists registries
/// (`footnote_refs`, `citation_refs`, `refnames`), whose keys and whose
/// values per key are both in registration order.
#[derive(Debug, Clone)]
pub struct OrderedMulti<T> {
    order: Vec<(String, Vec<T>)>,
    index: HashMap<String, usize>,
}

impl<T> Default for OrderedMulti<T> {
    fn default() -> Self {
        OrderedMulti {
            order: Vec::new(),
            index: HashMap::new(),
        }
    }
}

impl<T> OrderedMulti<T> {
    /// `registry.setdefault(key, []).append(value)`.
    pub fn push(&mut self, key: &str, value: T) {
        let slot = match self.index.get(key) {
            Some(&slot) => slot,
            None => {
                self.index.insert(key.to_string(), self.order.len());
                self.order.push((key.to_string(), Vec::new()));
                self.order.len() - 1
            }
        };
        self.order[slot].1.push(value);
    }

    /// `registry.get(key)`.
    pub fn get(&self, key: &str) -> Option<&[T]> {
        self.index
            .get(key)
            .map(|&slot| self.order[slot].1.as_slice())
    }

    /// Every key and its values, keys in first-insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[T])> {
        self.order
            .iter()
            .map(|(key, values)| (key.as_str(), values.as_slice()))
    }
}

/// The docutils `document` object state a parse leaves behind, which the
/// read transforms read beside the tree ([DS §8.1]). One per document: the
/// parser threads it through every nested and detached sub-parse and hands
/// it on as `ParseOutput.state`.
#[derive(Debug, Default, Clone)]
pub struct DocumentState {
    /// `document.ids` keys and `id_counter`.
    pub ids: IdAllocator,
    /// `document.nameids` and `nametypes`, in registration order.
    pub names: NameTable,
    /// The document's record sequence counter: parse-time records are
    /// ordered by the numbers they draw from it ([DS §6.4]).
    pub next_seq: u32,
    /// What only the parse reads ([`ParseOnlyState`]); empty once the
    /// parse has ended.
    pub(crate) parse: ParseOnlyState,
}

/// What the parser threads through every nested and detached sub-parse
/// beside the docutils object state, so that a sub-parse continues the
/// document rather than starting a new one: the record streams
/// `ParseOutput` carries, the dupnames still owed to nodes the parser has
/// placed, Sphinx's per-document serial counters, docutils' `include_log`
/// and substitution names, and the `note_pending` counter. The parse
/// drains it when it ends.
#[derive(Debug, Default, Clone)]
pub(crate) struct ParseOnlyState {
    /// docutils `dupname(old_node, name)` on nodes already placed in the
    /// tree, applied when the tree is complete
    /// ([`crate::doctree::ids::apply_dupname_fixups`]).
    pub(crate) dupname_fixups: Vec<DupnameFixup>,
    /// sphinx `env.new_serialno('index')`, shared by the index directive
    /// and the roles that emit index entries.
    pub(crate) index_serial: u32,
    /// The other `env.new_serialno(category)` counters, keyed by the
    /// `sphinx.util.nodes.make_id` prefix that asked for one
    /// (`cmdoption-myprog`, `confval`, …): per document and category, from
    /// 0, like [`Self::index_serial`].
    pub(crate) serialnos: HashMap<String, u32>,
    /// docutils `document.include_log` (`nodes.py:1802-1803`): the open
    /// inclusions as `(source display path, clip options)` pairs. Seeded
    /// with the root document (and an empty clip) on first use
    /// (`misc.py:253-256`); an entry is pushed before each splice and
    /// popped when the comment path reaches the matching
    /// `.. end of inclusion from "..."` marker.
    pub(crate) include_log: Vec<(String, IncludeClip)>,
    /// The substitution names defined so far (whitespace-normalized,
    /// case-preserving): docutils `document.substitution_defs`' keys.
    pub(crate) substitution_names_seen: Vec<String>,
    /// Names defined more than once: when the tree is complete, every
    /// definition but the last loses its name (docutils `dupname`s the old
    /// node in place).
    pub(crate) substitution_dupnames: Vec<String>,
    /// The next `document.note_pending` order
    /// ([`crate::doctree::pending::Pending::order`]).
    pub(crate) next_pending_order: u32,
    /// Validation-feed records ([`DirectiveRecord`], [`RoleRecord`]) and
    /// the toctree records.
    pub(crate) directive_records: Vec<DirectiveRecord>,
    pub(crate) role_records: Vec<RoleRecord>,
    pub(crate) toctree_records: Vec<ToctreeRecord>,
    /// The std- and py-domain registrations the parse made, in document
    /// order (see [`crate::rst::RegistryExport::program_options`] for why
    /// the finished doctree cannot carry them).
    pub(crate) program_option_records: Vec<ProgramOptionRecord>,
    pub(crate) std_object_records: Vec<ObjectRegistration>,
    pub(crate) py_object_records: Vec<PyObjectRecord>,
    pub(crate) py_module_records: Vec<PyModuleRecord>,
    /// `logger.warning` diagnostics raised while running directives.
    pub(crate) log_warnings: Vec<ParseLogWarning>,
    /// Parse-recorded dependency paths and included docnames (see
    /// [`crate::rst::RegistryExport::dependencies`]); sphinx mode only.
    pub(crate) dependency_records: Vec<String>,
    pub(crate) included_records: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctree::Span;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// docutils `document.set_id` under Sphinx's settings ([DS §5.1,
    /// §5.6]). The first of the node's names whose `make_id` is non-empty
    /// and free wins; otherwise the one shared counter hands out the next
    /// free `id<N>`, stepping over ids registered some other way. The
    /// second half replays the allocation order of [DS §2.2]'s
    /// `cases/pending` (the parse, then TargetNotes at 540, then Contents
    /// at 720), call for call as a `set_id` trace of its Sphinx 9.1.0 build
    /// records them: Contents asks `set_id` for a `toc-entry` prefix, which
    /// `auto_id_prefix = 'id'` ignores, so its three references take
    /// `id5 id6 id7`.
    #[test]
    fn id_allocator_matches_set_id() {
        let mut ids = IdAllocator::default();
        // `!!!` makes an empty id; the next name's is free.
        assert_eq!(
            ids.allocate(kinds::SECTION, &strings(&["!!!", "Hello World!"])),
            "hello-world"
        );
        // A taken name passes to the next one.
        assert_eq!(
            ids.allocate(kinds::TARGET, &strings(&["hello world", "second"])),
            "second"
        );
        // Every candidate taken: the counter.
        assert_eq!(
            ids.allocate(kinds::SECTION, &strings(&["Hello World!"])),
            "id1"
        );
        // A registered id is taken, and registering it again is the
        // duplicate case.
        assert!(!ids.is_taken("id2"));
        assert!(ids.register("id2"));
        assert!(!ids.register("id2"));
        assert!(ids.is_taken("id2"));
        // The counter steps over it, and over an auto-looking id a name
        // took first.
        assert_eq!(ids.allocate(kinds::PROBLEMATIC, &[]), "id3");
        assert_eq!(ids.allocate(kinds::TARGET, &strings(&["id4"])), "id4");
        assert_eq!(ids.allocate(kinds::SYSTEM_MESSAGE, &[]), "id5");

        let pending_case: [(&'static str, &[&str], &str); 13] = [
            (kinds::SECTION, &["doc"], "doc"),
            ("topic", &["contents"], "contents"),
            // `[#]_` in `Sec A`'s title, parsed before its section.
            (kinds::FOOTNOTE_REFERENCE, &[], "id1"),
            (kinds::SECTION, &["sec a"], "sec-a"),
            (kinds::TARGET, &["python"], "python"),
            (kinds::TARGET, &["docs"], "docs"),
            (kinds::SECTION, &["sec b"], "sec-b"),
            (kinds::FOOTNOTE, &[], "id2"),
            // TargetNotes (540): the note footnote, then its reference.
            (kinds::FOOTNOTE, &[], "id3"),
            (kinds::FOOTNOTE_REFERENCE, &[], "id4"),
            // Contents (720): one id per toc reference.
            (kinds::REFERENCE, &[], "id5"),
            (kinds::REFERENCE, &[], "id6"),
            (kinds::REFERENCE, &[], "id7"),
        ];
        let mut ids = IdAllocator::default();
        for (kind, names, expected) in pending_case {
            assert_eq!(ids.allocate(kind, &strings(names)), expected, "{kind}");
        }
    }

    /// A `target` named `x` with neither `refuri` nor `refname`.
    fn bind_x(table: &mut NameTable, id: &str, explicit: bool) -> Vec<DupOutcome> {
        let names = strings(&["x"]);
        let view = NodeView {
            kind: kinds::TARGET,
            names: &names,
            refuri: None,
            refname: None,
        };
        table.bind(view, id, explicit)
    }

    fn outcome(
        name: &str,
        level: u8,
        message: &str,
        backrefs: bool,
        dupname_new: bool,
        dupname_old: Option<&str>,
    ) -> DupOutcome {
        DupOutcome {
            name: name.to_string(),
            level,
            message: message.to_string(),
            backrefs,
            dupname_new,
            dupname_old: dupname_old.map(str::to_string),
        }
    }

    /// docutils' `set_name_id_map` / `set_duplicate_name_id`
    /// (`SP/docutils/nodes.py:1877-1990`), probed on docutils 0.22.4. First
    /// every row of the state table at `nodes.py:1897-1912`: the old state
    /// is reached by binding `x` once or twice, the row's input binds it
    /// again as `new`, and the table's new state and actions (which node
    /// loses the name, which level reports) are checked. Then the branch
    /// the table's footnote describes ([DS §7.5 (1)]): a duplicate target
    /// that refers to the same `refname` or `refuri` as the bound one
    /// keeps the first and is only an INFO.
    #[test]
    fn name_table_bind_matches_docutils() {
        const EXPLICIT_DUP: &str = "Duplicate explicit target name: \"x\".";
        const IMPLICIT_DUP: &str = "Duplicate implicit target name: \"x\".";
        const OVERRIDE: &str = "Target name overrides implicit target name \"x\".";
        let dup = |level, message, dupname_new, dupname_old| {
            vec![outcome("x", level, message, true, dupname_new, dupname_old)]
        };
        // (input explicit, old bindings' explicitness, then nameids[x],
        // nametypes[x] and the reported duplicates)
        type Row = (
            bool,
            &'static [bool],
            Option<&'static str>,
            bool,
            Vec<DupOutcome>,
        );
        let table_rows: [Row; 10] = [
            (true, &[], Some("new"), true, vec![]),
            (false, &[], Some("new"), false, vec![]),
            (
                true,
                &[true],
                None,
                true,
                dup(WARNING, EXPLICIT_DUP, true, Some("old1")),
            ),
            (
                false,
                &[true],
                Some("old1"),
                true,
                dup(INFO, IMPLICIT_DUP, true, None),
            ),
            (
                true,
                &[false],
                Some("new"),
                true,
                dup(INFO, OVERRIDE, false, Some("old1")),
            ),
            (
                false,
                &[false],
                None,
                false,
                dup(INFO, IMPLICIT_DUP, true, Some("old1")),
            ),
            (
                true,
                &[true, true],
                None,
                true,
                dup(WARNING, EXPLICIT_DUP, true, None),
            ),
            (
                false,
                &[true, true],
                None,
                true,
                dup(INFO, IMPLICIT_DUP, true, None),
            ),
            // No report and no dupname: the explicit target takes the name.
            (true, &[false, false], Some("new"), true, vec![]),
            (
                false,
                &[false, false],
                None,
                false,
                dup(INFO, IMPLICIT_DUP, true, None),
            ),
        ];
        for (row, (explicit, old, id, now_explicit, reported)) in table_rows.into_iter().enumerate()
        {
            let row = row + 1;
            let mut table = NameTable::default();
            for (i, &old_explicit) in old.iter().enumerate() {
                bind_x(&mut table, &format!("old{}", i + 1), old_explicit);
            }
            assert_eq!(bind_x(&mut table, "new", explicit), reported, "row {row}");
            let entry = table.get("x").expect("bound");
            assert_eq!(
                (entry.id.as_deref(), entry.explicit),
                (id, now_explicit),
                "row {row}"
            );
        }

        // `.. _a: x_` twice (`cases/dup_indirect`): the second keeps the
        // first bound and is an INFO; the new id is the backref.
        let names = strings(&["a"]);
        let indirect = |refname| NodeView {
            kind: kinds::TARGET,
            names: &names,
            refuri: None,
            refname: Some(refname),
        };
        let mut table = NameTable::default();
        assert!(table.bind(indirect("x"), "a", true).is_empty());
        assert_eq!(
            table.bind(indirect("x"), "id1", true),
            vec![outcome(
                "a",
                INFO,
                "Duplicate name \"a\" for external target \"x\".",
                true,
                true,
                None
            )]
        );
        assert_eq!(
            table.get("a"),
            Some(&NameEntry {
                id: Some("a".to_string()),
                explicit: true,
                refuri: None,
                refname: Some("x".to_string()),
            })
        );
        // Another refname is an ordinary explicit duplicate
        // (`.. _a: x_` / `.. _a: y_`).
        let mut table = NameTable::default();
        table.bind(indirect("x"), "a", true);
        assert_eq!(
            table.bind(indirect("y"), "id1", true),
            vec![outcome(
                "a",
                WARNING,
                "Duplicate explicit target name: \"a\".",
                true,
                true,
                Some("a")
            )]
        );

        // The same refuri (`.. _dup: https://1/` twice): the same branch,
        // and a target with a `refuri` gets no backref. The views come from
        // parsed-shape nodes.
        let target = |refuri: &str| {
            let mut node = Node::elem(kinds::TARGET, Span::ZERO);
            node.attrs.names.push("dup".to_string());
            node.set("refuri", AttrValue::Str(refuri.to_string()));
            node
        };
        let (first, second, other) = (
            target("https://1/"),
            target("https://1/"),
            target("https://2/"),
        );
        let mut table = NameTable::default();
        assert!(table.bind(NodeView::of(&first), "dup", true).is_empty());
        assert_eq!(
            table.bind(NodeView::of(&second), "id1", true),
            vec![outcome(
                "dup",
                INFO,
                "Duplicate name \"dup\" for external target \"https://1/\".",
                false,
                true,
                None
            )]
        );
        // A different refuri clears the name; after that the same refuri
        // as the first no longer matches anything (`.. _dup:` https://1/,
        // https://2/, https://1/).
        let mut table = NameTable::default();
        table.bind(NodeView::of(&first), "dup", true);
        assert_eq!(
            table.bind(NodeView::of(&other), "id1", true),
            vec![outcome(
                "dup",
                WARNING,
                "Duplicate explicit target name: \"dup\".",
                false,
                true,
                Some("dup")
            )]
        );
        assert_eq!(
            table.bind(NodeView::of(&second), "id2", true),
            vec![outcome(
                "dup",
                WARNING,
                "Duplicate explicit target name: \"dup\".",
                false,
                true,
                None
            )]
        );
        // An implicit (embedded) target with an explicit target's refuri
        // takes the branch too, before the implicit-duplicate rule.
        let mut table = NameTable::default();
        table.bind(NodeView::of(&first), "dup", true);
        assert_eq!(
            table.bind(NodeView::of(&second), "id1", false),
            vec![outcome(
                "dup",
                INFO,
                "Duplicate name \"dup\" for external target \"https://1/\".",
                false,
                true,
                None
            )]
        );
        assert_eq!(table.get("dup").map(|entry| entry.explicit), Some(true));

        // nametypes order is first-binding order (the `label_order` case
        // of [DS §7.4]: `zeta` before `alpha`).
        let mut table = NameTable::default();
        for (name, id, explicit) in [("zeta", "zeta", true), ("alpha", "alpha", true)] {
            let names = strings(&[name]);
            let view = NodeView {
                kind: kinds::TARGET,
                names: &names,
                refuri: None,
                refname: None,
            };
            table.bind(view, id, explicit);
        }
        bind_x(&mut table, "x", false);
        bind_x(&mut table, "id1", true);
        let order: Vec<&str> = table.iter().map(|(name, _)| name).collect();
        assert_eq!(order, ["zeta", "alpha", "x"]);
        assert!(table.has_name("alpha"));
        assert!(!table.has_name("beta"));
    }

    /// With the trace on, every `set_id` on a node without ids records its
    /// phase, the node's kind and the id; `register` records nothing.
    #[test]
    fn id_trace_records_kind_and_phase() {
        let source = "Title\n=====\n\n.. _tgt:\n\nBody.\n";
        let traced = crate::rst::parse_rst_full(
            source,
            &crate::rst::ParseOptions {
                trace_ids: true,
                ..Default::default()
            },
        );
        let events: Vec<(&str, &str, &str, bool)> = traced
            .state
            .ids
            .trace()
            .iter()
            .map(|event| {
                (
                    event.phase.as_str(),
                    event.kind,
                    event.id.as_str(),
                    event.attached,
                )
            })
            .collect();
        assert_eq!(
            events,
            [
                ("parse", "section", "title", false),
                ("parse", "target", "tgt", false)
            ]
        );
        let untraced = crate::rst::parse_rst_full(source, &crate::rst::ParseOptions::default());
        assert!(untraced.state.ids.trace().is_empty());

        let mut ids = traced.state.ids;
        assert!(ids.register("explicit-id"));
        ids.set_phase("docutils.transforms.references.Substitutions".to_string());
        assert_eq!(ids.allocate(kinds::PROBLEMATIC, &[]), "id1");
        let last = ids.trace().last().expect("recorded");
        assert_eq!(
            (last.phase.as_str(), last.kind, last.id.as_str()),
            (
                "docutils.transforms.references.Substitutions",
                "problematic",
                "id1"
            )
        );
        assert_eq!(ids.trace().len(), 3);
    }

    /// docutils' dict-of-lists registries: keys in first-insertion order,
    /// each key's values in insertion order.
    #[test]
    fn ordered_multi_keeps_registration_order() {
        let mut refs: OrderedMulti<u32> = OrderedMulti::default();
        refs.push("zeta", 1);
        refs.push("alpha", 2);
        refs.push("zeta", 3);
        assert_eq!(refs.get("zeta"), Some(&[1, 3][..]));
        assert_eq!(refs.get("beta"), None);
        let order: Vec<(&str, &[u32])> = refs.iter().collect();
        assert_eq!(order, [("zeta", &[1, 3][..]), ("alpha", &[2][..])]);
    }
}
