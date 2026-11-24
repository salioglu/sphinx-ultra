//! The docutils document state a parse leaves behind (wave 5a, decision 1;
//! [DS §8.1, §12.2]).
//!
//! Sphinx parses into one docutils `document` and then runs every read
//! transform over that same object, so the transforms read more than the
//! tree: every id the parse registered, those of loose messages and of
//! dropped nodes included, the auto-id counter, the name table in
//! registration order, and the node registries, which hold nodes by
//! identity, including nodes the parse moved or discarded.
//! [`DocumentState`] is that object state. The parser threads one through
//! every nested and detached sub-parse and hands it on as
//! `ParseOutput.state`.

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;

use crate::diagnostics::OrderKey;
use crate::doctree::ids::{fully_normalize_name, make_id};
use crate::doctree::messages::{self, INFO, WARNING};
use crate::doctree::pending::PendingTransform;
use crate::doctree::{kinds, AttrValue, Node, Uid};
use crate::rst::ParseOnlyState;
use crate::transforms::reporter::LogRecord;

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
    /// `document.ids`: every id the document registered, with the identity
    /// of the node holding it when that node has one ([`Uid`]). Ids of
    /// loose messages and of nodes the parse dropped are kept, because
    /// they still block reuse.
    taken: HashMap<String, Option<Uid>>,
    /// `document.id_counter['id']`, the one auto-id counter.
    counter: u32,
    /// One event per [`Self::allocate`] while enabled
    /// ([`Self::enable_trace`]).
    trace: Option<Vec<IdEvent>>,
    /// The phase new events record; `None` is the parse.
    phase: Option<String>,
    /// By trace event, the identity of the node its id named at the end of
    /// stage R, when that node was in the tree ([`Self::tag_event`]).
    tags: Vec<Option<Uid>>,
}

impl IdAllocator {
    /// docutils `document.set_id` for a node without ids
    /// (`SP/docutils/nodes.py:1844-1875`): the first of the node's `names`
    /// whose `make_id` is non-empty and free, else the next free `id<N>`
    /// from the shared counter. Under `auto_id_prefix = 'id'` the `'%'`
    /// branches are dead, so docutils' `suggested_prefix` never applies and
    /// is not taken ([DS §5.6]). The id is registered as `holder`'s, the
    /// node's identity when it has one; with the trace on, one [`IdEvent`]
    /// records it with the node's `kind`.
    pub fn allocate(
        &mut self,
        kind: &'static str,
        names: &[String],
        holder: Option<Uid>,
    ) -> String {
        let from_name = names
            .iter()
            .map(|name| make_id(name))
            .find(|base| !base.is_empty() && !self.taken.contains_key(base));
        let id = match from_name {
            Some(id) => id,
            None => loop {
                self.counter += 1;
                let id = format!("id{}", self.counter);
                if !self.taken.contains_key(&id) {
                    break id;
                }
            },
        };
        self.taken.insert(id.clone(), holder);
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
    /// (`nodes.py:1832-1843`), one id at a time: `ids.setdefault(id, node)`,
    /// with `holder` the registering node's identity when it has one.
    /// `false` is docutils' `Duplicate ID` case, `ids[id] is not node`:
    /// another node holds the id. Re-registering an id the same node
    /// already holds is fine, as in docutils, when that node has an
    /// identity. A node without identity cannot be told from another, so
    /// an id held without identity, or registered without one, answers
    /// `false` once taken. The precondition that keeps a re-registration
    /// right is therefore on the caller: a node docutils `set_id`s more
    /// than once (a registered node: a `[#name]_` reference, a footnote,
    /// an indirect target) has its identity minted before its first
    /// `set_id`. Never traced: the fixture's wrapper records only nodes
    /// without ids.
    pub fn register(&mut self, id: &str, holder: Option<Uid>) -> bool {
        match self.taken.get(id) {
            None => {
                self.taken.insert(id.to_string(), holder);
                true
            }
            Some(held) => holder.is_some() && *held == holder,
        }
    }

    /// `id in document.ids`.
    pub fn is_taken(&self, id: &str) -> bool {
        self.taken.contains_key(id)
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

    /// Tags trace event `event` (its index in [`Self::trace`]) with `uid`,
    /// the identity of the node its id names: stage R tags every event
    /// whose node is in the tree when the stage ends, so that once the
    /// tree is final the read can tell whether that node is still in it
    /// ([`IdEvent::attached`]).
    pub fn tag_event(&mut self, event: usize, uid: Uid) {
        debug_assert!(event < self.trace().len(), "no trace event {event}");
        if self.tags.len() <= event {
            self.tags.resize(event + 1, None);
        }
        self.tags[event] = Some(uid);
    }

    /// The identity [`Self::tag_event`] tagged trace event `event` with;
    /// `None` for an event whose node was out of the tree, or nowhere, when
    /// stage R ended.
    pub fn event_tag(&self, event: usize) -> Option<Uid> {
        self.tags.get(event).copied().flatten()
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

/// A source a docutils location names: an entry of the document's source
/// table (`Doctree::sources`). Included files are entries of their own,
/// and so is the padding line docutils inserts after each inclusion
/// (`internal padding after <path>`), the source of a document that ends
/// inside an include ([DS §6.2]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRef {
    Table(u16),
}

/// A docutils location, `(source, line)`, either half of which can be
/// missing: a message docutils could not place has neither, and a node
/// the parser appended right after a nested state machine ended has a
/// source and no line ([DS §6.1, §8.2]). Lines are 1-based.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DocLoc {
    pub source: Option<SourceRef>,
    pub line: Option<u32>,
}

impl DocLoc {
    /// Line `line` of source-table entry `source`.
    pub fn at(source: u16, line: u32) -> DocLoc {
        DocLoc {
            source: Some(SourceRef::Table(source)),
            line: Some(line),
        }
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
    /// The document's record sequence counter ([`Self::draw_seq`]): one
    /// count across reporter records, `ParseLogWarning`s, toctree warnings
    /// and the parse-time registration records, so the parse's records
    /// are ordered by the numbers they draw ([DS §6.4]).
    pub next_seq: u32,
    /// The reporter log: one record per message, at creation, in creation
    /// order ([`Self::report`]).
    pub log: Vec<LogRecord>,
    /// docutils' `reporter.get_source_and_line(None)` once the parse has
    /// ended: the top-level state machine's final position, which a
    /// message with neither a node nor a line is located at ([DS §6.2]).
    pub final_position: DocLoc,
    /// The top-level state machine's `input_lines` once the parse has
    /// ended: each line's source-table entry and 1-based line, as
    /// `input_lines.info(offset)` gives them (`srcoffset + 1`; 0 for an
    /// `internal padding before` line). An include at any depth has put
    /// its lines and their padding lines in, where `ViewList.insert` passes
    /// them up to: after the last line of the include's block, trailing
    /// blank lines included unless they ended a directive's content. An
    /// include in a table cell, a glossary definition, a substitution
    /// definition or the content of a directive that takes options but no
    /// arguments, when that content starts on the directive line or right
    /// below it (a `note`'s, say, which `parse_directive_block` re-joins as
    /// `arg_block + indented[i:]`), has not, since docutils parses those
    /// from lists of their own. What a message given a line after the parse
    /// is located against ([`Self::source_and_line`]).
    pub input_lines: Vec<(u16, u32)>,
    /// The docutils locations of the nodes whose location the default rule
    /// ([DS §8.2]) does not give, by identity, as docutils' parse leaves
    /// them: the location model completes one without a source as Sphinx's
    /// ApplySourceWorkaround (010) does
    /// ([`crate::transforms::location`]). So far:
    ///
    /// - the `reference` an `image`'s `:target:` wraps: docutils appends it
    ///   after the option block's nested state machine has cleared the
    ///   line, so it has the source and no line when its container has
    ///   `document` set, and neither when it has not ([DS §6.1]);
    /// - a `parsed-literal`'s `literal_block`, which stands at its first
    ///   content line, with no source in a container without `document`;
    /// - a `rubric`, which stands where the machine reading its block
    ///   stopped, has the source and no line after an option block, and
    ///   has neither in a container without `document`;
    /// - each `line` of a `line-block` directive, at its block's location
    ///   moved on by the line's index in the content;
    /// - a `classifier` docutils makes with an empty rawsource, at the
    ///   location 010 gives it: its item's source and the line before its
    ///   item's, which only the rawsource the parse saw decides.
    ///
    /// A node the parse discards keeps its entry only inside a subtree
    /// [`Self::detached`] keeps.
    pub loc_overrides: HashMap<Uid, DocLoc>,
    /// The source-table entries whose nodes docutils locates elsewhere than
    /// their spans say, with where: the lines of a csv-table cell, which
    /// docutils parses as `StringList(cell.splitlines(), source=source)`
    /// (`SP/docutils/parsers/rst/directives/tables.py:398-399`), so a node
    /// its nested machine stamps stands at the csv source's line `1 +` its
    /// index in the cell. Keyed by the cell's entry; the value is the csv
    /// source and the line the cell's first line has here.
    pub span_remaps: HashMap<u16, (SourceRef, u32)>,
    /// docutils' node registries ([`Registries`]), by node identity.
    pub registries: Registries,
    /// The transforms `document.note_pending` queued, in parse order.
    pub pending: Vec<PendingRecord>,
    /// The subtrees the parser discarded that hold a node a registry or a
    /// pending record holds: in docutils those nodes leave the tree but
    /// stay in the registries, where the transforms still number them,
    /// resolve through them and report against them ([DS §4.4]). Every registered [`Uid`] is reachable exactly once
    /// from the root or from here ([`Self::misplaced_registrations`]). The
    /// read transforms' arena takes the forest over
    /// ([`crate::transforms::tree::TxTree::from_parse`]), leaving this
    /// empty.
    pub detached: Vec<Node>,
    /// How many identities the document has minted ([`Self::mint_uid`]).
    pub next_uid: u32,
    /// What the citation transforms' tree halves (619) leave the citation
    /// domain, whose stage-M half registers it
    /// ([`crate::env::citation`]): the citations
    /// `CitationDefinitionTransform` found in the tree and the target of
    /// each `pending_xref` `CitationReferenceTransform` made, both in tree
    /// order.
    pub citation_notes: Vec<CitationNote>,
    pub citation_reference_targets: Vec<String>,
    /// What only the parse reads ([`ParseOnlyState`]); empty once the
    /// parse has ended.
    pub(crate) parse: ParseOnlyState,
}

/// docutils' node registries on `document` (`nodes.py:1996-2089`), each in
/// registration order and holding node identities, so registered nodes
/// that the parse later moved or discarded keep their place ([DS §4.4,
/// §8.1]). The `note_*` calls that fill them are the parser's
/// ([`crate::doctree::ids`]); the read transforms consume them.
#[derive(Debug, Default, Clone)]
pub struct Registries {
    /// `note_autofootnote` / `note_autofootnote_ref`: `[#]` and `[#name]`.
    pub autofootnotes: Vec<Uid>,
    pub autofootnote_refs: Vec<Uid>,
    /// `note_symbol_footnote` / `note_symbol_footnote_ref`: `[*]`.
    pub symbol_footnotes: Vec<Uid>,
    pub symbol_footnote_refs: Vec<Uid>,
    /// `note_footnote`: manually numbered footnotes.
    pub footnotes: Vec<Uid>,
    /// `note_footnote_ref`: references with a `refname`, by it.
    pub footnote_refs: OrderedMulti<Uid>,
    /// `note_citation` / `note_citation_ref` (the references by
    /// `refname`).
    pub citations: Vec<Uid>,
    pub citation_refs: OrderedMulti<Uid>,
    /// `note_refname`: every node noted under its `refname`.
    pub refnames: OrderedMulti<Uid>,
    /// `note_indirect_target`: targets that refer to another name.
    pub indirect_targets: Vec<Uid>,
    /// `note_substitution_def`, one entry per call: docutils' dict keeps a
    /// redefined name at its first position and maps it to the last
    /// definition.
    pub substitution_defs: Vec<(String, Uid)>,
    /// `substitution_names`: the fully normalized name of each definition
    /// to its name as defined (last definition wins).
    pub substitution_names: HashMap<String, String>,
}

impl Registries {
    /// `name in document.substitution_defs`.
    pub fn has_substitution_def(&self, name: &str) -> bool {
        self.substitution_defs
            .iter()
            .any(|(defined, _)| defined == name)
    }

    /// Every registry entry's identity, in the order
    /// [`DocumentState::registries_dump`] lists the registries (a node
    /// several registries hold appears once per registry).
    fn uids(&self) -> Vec<Uid> {
        let mut uids = Vec::new();
        for list in [
            &self.autofootnotes,
            &self.autofootnote_refs,
            &self.symbol_footnotes,
            &self.symbol_footnote_refs,
            &self.footnotes,
        ] {
            uids.extend(list);
        }
        let multi = |uids: &mut Vec<Uid>, registry: &OrderedMulti<Uid>| {
            for (_, values) in registry.iter() {
                uids.extend(values);
            }
        };
        multi(&mut uids, &self.footnote_refs);
        uids.extend(&self.citations);
        multi(&mut uids, &self.citation_refs);
        multi(&mut uids, &self.refnames);
        uids.extend(&self.indirect_targets);
        uids.extend(self.substitution_defs.iter().map(|(_, uid)| *uid));
        uids
    }
}

/// One citation `CitationDefinitionTransform` (619-016) hands the citation
/// domain's `note_citation` (`SP/sphinx/domains/citation.py:69-82`): its
/// label (`node[0].astext()`), its ids and the node itself, whose
/// location the domain's duplicate warning and its record take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationNote {
    pub label: String,
    pub ids: Vec<String>,
    pub node: Uid,
}

/// One `document.note_pending` call: the transform a `pending` node
/// defers, its parse order among the pending transforms (which orders
/// pending transforms of equal priority, [DS §2.2]) and the node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingRecord {
    pub transform: PendingTransform,
    /// 0-based `note_pending` order ([`crate::doctree::pending::Pending::order`]).
    pub order: u32,
    pub node: Uid,
}

impl DocumentState {
    /// A new identity, never reused within the document: `next_uid` is
    /// incremented first, so a default state mints 1 first.
    pub fn mint_uid(&mut self) -> Uid {
        self.next_uid += 1;
        Uid::new(NonZeroU32::new(self.next_uid).expect("a Uid counter past zero"))
    }

    /// docutils' `reporter.get_source_and_line(lineno)` once the parse has
    /// ended, which the reporter binds to the top-level state machine
    /// (`SP/docutils/parsers/rst/states.py:245-246`;
    /// `StateMachine.get_source_and_line`, `SP/docutils/statemachine.py:358-383`):
    /// for no line, the machine's final position ([`Self::final_position`]);
    /// for a line, the location of the input line at offset `lineno - 1`
    /// ([`Self::input_lines`]), whatever source the line was counted in.
    /// Python's indexing applies: an offset just past the end is the last
    /// line's source and its line + 1, one further past is neither, and a
    /// negative one counts from the end.
    pub fn source_and_line(&self, lineno: Option<u32>) -> DocLoc {
        let Some(lineno) = lineno else {
            return self.final_position;
        };
        let len = self.input_lines.len() as i64;
        let offset = i64::from(lineno) - 1;
        let index = if offset < 0 { offset + len } else { offset };
        match usize::try_from(index)
            .ok()
            .and_then(|index| self.input_lines.get(index))
        {
            Some(&(source, line)) => DocLoc::at(source, line),
            None if offset == len => match self.input_lines.last() {
                Some(&(source, line)) => DocLoc::at(source, line + 1),
                None => DocLoc::default(),
            },
            None => DocLoc::default(),
        }
    }

    /// The next record sequence number, from 0 ([`Self::next_seq`]).
    pub fn draw_seq(&mut self) -> u32 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    /// A Sphinx `logger.warning` the parse raises: it rides the parse's
    /// `log_warnings` (never the tree), with the next record seq.
    pub(crate) fn log_warning(&mut self, message: String, location: crate::rst::LogLocation) {
        let seq = self.draw_seq();
        self.parse.log_warnings.push(crate::rst::ParseLogWarning {
            message,
            location,
            seq,
        });
    }

    /// docutils `Reporter.system_message(level, text, *children, line=…)`
    /// during the parse (`SP/docutils/utils/__init__.py:171-221`): builds
    /// the `system_message` node at `loc` — its `level`, `line` (when there
    /// is one), `source` (the source table's path; the document's own
    /// when `loc` names none, docutils' `reporter.source`) and `type`, a
    /// paragraph holding `text`, then `children` — and logs it. The record
    /// ([`LogRecord::Reporter`], keyed `(0, 0, seq)` as every parse record
    /// is) holds the node's `Element.astext()` as it is now: docutils
    /// writes the message to its stream right here, so children a caller
    /// appends later are not in the text it prints ([TW §5.3.2]).
    pub fn report(&mut self, level: u8, text: &str, loc: DocLoc, children: Vec<Node>) -> Node {
        let source = match loc.source {
            Some(SourceRef::Table(source)) => source,
            None => 0,
        };
        let node = messages::system_message(
            level,
            text,
            self.parse.source_path(source),
            loc.line,
            children,
        );
        let seq = self.draw_seq();
        self.log.push(LogRecord::Reporter {
            key: OrderKey {
                priority: 0,
                serial: 0,
                seq,
            },
            level,
            loc,
            text: node.element_astext(),
        });
        node
    }

    /// Subtrees the parser discards. Each one holding a node a registry or
    /// a pending record holds is kept in [`Self::detached`], the way
    /// docutils' registries keep such nodes alive. The rest are dropped,
    /// as docutils garbage-collects them, together with the location
    /// overrides of their nodes ([`Self::loc_overrides`]): a node with an
    /// identity for its override alone keeps nothing alive.
    pub(crate) fn detach(&mut self, nodes: impl IntoIterator<Item = Node>) {
        let mut held: Option<HashSet<Uid>> = None;
        for node in nodes {
            let mut uids = Vec::new();
            identities(&node, &mut uids);
            if uids.is_empty() {
                continue;
            }
            let held = held.get_or_insert_with(|| self.held_uids().collect());
            if uids.iter().any(|uid| held.contains(uid)) {
                self.detached.push(node);
            } else {
                for uid in &uids {
                    self.loc_overrides.remove(uid);
                }
            }
        }
    }

    /// The read-state fixture's `registries_post_parse` ([DS §11]) for the
    /// tree under `root`. Each registered node is an entry
    /// `[kind, ids, attached]`: its tagname, its ids as they are now, and
    /// whether it is reachable from `root` (`false` when only
    /// [`Self::detached`] holds it; `null` for a node neither holds). The
    /// object has one key per registry, every key always present:
    ///
    /// - `autofootnotes`, `autofootnote_refs`, `symbol_footnotes`,
    ///   `symbol_footnote_refs`, `footnotes`, `citations`,
    ///   `indirect_targets`: lists of entries, in registration order;
    /// - `footnote_refs`, `citation_refs`, `refnames`: `[key, entries]`
    ///   pairs, keys in first-registration order;
    /// - `substitution_defs`: `[name, entry]` pairs and
    ///   `substitution_names`: `[normalized name, name]` pairs, both in
    ///   docutils' dict order (a redefined name keeps its first position
    ///   and takes its last value);
    /// - `names`: `[name, id or null, explicit]`, in `nametypes` order.
    ///
    /// The pending transforms ([`Self::pending`]) are not a registry and
    /// are not dumped.
    ///
    /// The dump is a parse's: a read's stage R hands the detached forest to
    /// its arena ([`crate::transforms::tree::TxTree::from_parse`]), after
    /// which the state holds none of it and a detached node dumps as
    /// `null`, so the read-state differential dumps the state
    /// `parse_rst_full` returns.
    pub fn registries_dump(&self, root: &Node) -> serde_json::Value {
        use serde_json::{json, Value};
        let reached = Reached::of(root, &self.detached);
        let entry = |uid: &Uid| reached.entry(*uid);
        let list = |uids: &[Uid]| Value::Array(uids.iter().map(entry).collect());
        let multi = |registry: &OrderedMulti<Uid>| {
            Value::Array(
                registry
                    .iter()
                    .map(|(key, uids)| json!([key, list(uids)]))
                    .collect(),
            )
        };
        let registries = &self.registries;
        // docutils' dicts: first insertion fixes a key's position, the
        // last assignment its value.
        let mut defs: Vec<(&str, Uid)> = Vec::new();
        let mut normalized: Vec<String> = Vec::new();
        for (name, uid) in &registries.substitution_defs {
            match defs.iter_mut().find(|(defined, _)| defined == name) {
                Some(slot) => slot.1 = *uid,
                None => defs.push((name, *uid)),
            }
            let key = fully_normalize_name(name);
            if !normalized.contains(&key) {
                normalized.push(key);
            }
        }
        json!({
            "autofootnotes": list(&registries.autofootnotes),
            "autofootnote_refs": list(&registries.autofootnote_refs),
            "symbol_footnotes": list(&registries.symbol_footnotes),
            "symbol_footnote_refs": list(&registries.symbol_footnote_refs),
            "footnotes": list(&registries.footnotes),
            "footnote_refs": multi(&registries.footnote_refs),
            "citations": list(&registries.citations),
            "citation_refs": multi(&registries.citation_refs),
            "refnames": multi(&registries.refnames),
            "indirect_targets": list(&registries.indirect_targets),
            "substitution_defs": defs
                .iter()
                .map(|(name, uid)| json!([name, entry(uid)]))
                .collect::<Vec<_>>(),
            "substitution_names": normalized
                .iter()
                .map(|key| json!([key, registries.substitution_names.get(key)]))
                .collect::<Vec<_>>(),
            "names": self
                .names
                .iter()
                .map(|(name, bound)| json!([name, bound.id, bound.explicit]))
                .collect::<Vec<_>>(),
        })
    }

    /// Every identity the state holds a node by: each registry entry's (a
    /// node several registries hold comes once per registry) and each
    /// pending record's.
    pub(crate) fn held_uids(&self) -> impl Iterator<Item = Uid> + '_ {
        self.registries
            .uids()
            .into_iter()
            .chain(self.pending.iter().map(|record| record.node))
    }

    /// The end-of-parse invariant ([DS §8.2]): every registered identity
    /// (the registries' and the pending records') is reachable exactly
    /// once from `root` or from [`Self::detached`]. Returns each one that
    /// is not, with how many times it was reached (0: the parse dropped a
    /// registered node; 2 or more: it copied one), in minting order. Like
    /// [`Self::registries_dump`], it checks a parse's state: the parse runs
    /// it before handing the state on, ahead of any read transform.
    pub fn misplaced_registrations(&self, root: &Node) -> Vec<(Uid, usize)> {
        let reached = Reached::of(root, &self.detached);
        let mut seen = HashSet::new();
        let mut misplaced: Vec<(Uid, usize)> = self
            .held_uids()
            .filter(|uid| seen.insert(*uid))
            .map(|uid| (uid, reached.count(uid)))
            .filter(|&(_, count)| count != 1)
            .collect();
        misplaced.sort_by_key(|&(uid, _)| uid);
        misplaced
    }

    /// How many [`Self::loc_overrides`] entries name a node neither the tree
    /// under `root` nor [`Self::detached`] holds. None at the end of a
    /// parse: [`Self::detach`] drops a dropped node's override with it, so
    /// an override always has its node. One that had not would be inert,
    /// since nothing reaches it, but it is state the parse lost track of.
    pub fn unreachable_overrides(&self, root: &Node) -> usize {
        let reached = Reached::of(root, &self.detached);
        self.loc_overrides
            .keys()
            .filter(|&&uid| reached.count(uid) == 0)
            .count()
    }
}

/// Every identity in the subtree, in pre-order.
fn identities(node: &Node, out: &mut Vec<Uid>) {
    out.extend(node.uid());
    for child in &node.children {
        identities(child, out);
    }
}

/// Where each identity sits in a tree and its detached forest: the node
/// first found carrying it, whether that was under the root, and how many
/// nodes carry it.
struct Reached<'a> {
    nodes: HashMap<Uid, (&'a Node, bool, usize)>,
}

impl<'a> Reached<'a> {
    fn of(root: &'a Node, detached: &'a [Node]) -> Reached<'a> {
        fn walk<'a>(
            node: &'a Node,
            attached: bool,
            nodes: &mut HashMap<Uid, (&'a Node, bool, usize)>,
        ) {
            if let Some(uid) = node.uid() {
                nodes
                    .entry(uid)
                    .and_modify(|(_, _, count)| *count += 1)
                    .or_insert((node, attached, 1));
            }
            for child in &node.children {
                walk(child, attached, nodes);
            }
        }
        let mut nodes = HashMap::new();
        walk(root, true, &mut nodes);
        for node in detached {
            walk(node, false, &mut nodes);
        }
        Reached { nodes }
    }

    fn count(&self, uid: Uid) -> usize {
        self.nodes.get(&uid).map_or(0, |&(_, _, count)| count)
    }

    /// `[kind, ids, attached]`, or `null` for an identity nothing carries.
    fn entry(&self, uid: Uid) -> serde_json::Value {
        match self.nodes.get(&uid) {
            Some((node, attached, _)) => serde_json::json!([node.kind, node.attrs.ids, attached]),
            None => serde_json::Value::Null,
        }
    }
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
            ids.allocate(kinds::SECTION, &strings(&["!!!", "Hello World!"]), None),
            "hello-world"
        );
        // A taken name passes to the next one.
        assert_eq!(
            ids.allocate(kinds::TARGET, &strings(&["hello world", "second"]), None),
            "second"
        );
        // Every candidate taken: the counter.
        assert_eq!(
            ids.allocate(kinds::SECTION, &strings(&["Hello World!"]), None),
            "id1"
        );
        // A registered id is taken, and registering it again is the
        // duplicate case.
        assert!(!ids.is_taken("id2"));
        assert!(ids.register("id2", None));
        assert!(!ids.register("id2", None));
        assert!(ids.is_taken("id2"));
        // The counter steps over it, and over an auto-looking id a name
        // took first.
        assert_eq!(ids.allocate(kinds::PROBLEMATIC, &[], None), "id3");
        assert_eq!(ids.allocate(kinds::TARGET, &strings(&["id4"]), None), "id4");
        assert_eq!(ids.allocate(kinds::SYSTEM_MESSAGE, &[], None), "id5");

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
            assert_eq!(
                ids.allocate(kind, &strings(names), None),
                expected,
                "{kind}"
            );
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
        assert!(ids.register("explicit-id", None));
        ids.set_phase("docutils.transforms.references.Substitutions".to_string());
        assert_eq!(ids.allocate(kinds::PROBLEMATIC, &[], None), "id1");
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

    fn uid(n: u32) -> Uid {
        Uid::new(NonZeroU32::new(n).expect("a non-zero uid"))
    }

    /// docutils `set_id` registers an id with `ids.setdefault(id, node)`
    /// and reports `Duplicate ID` only when `ids[id] is not node`
    /// (`SP/docutils/nodes.py:1832-1843`): a node registering an id it
    /// already holds is fine. `[#name]_` does exactly that (probed:
    /// `set_id` allocates `id1` for the reference in
    /// `note_autofootnote_ref`, then registers `id1` again in
    /// `note_footnote_ref`), and so does every footnote definition.
    #[test]
    fn id_allocator_register_is_identity_aware() {
        let mut ids = IdAllocator::default();
        let (holder, other) = (uid(1), uid(2));
        assert_eq!(
            ids.allocate(kinds::FOOTNOTE_REFERENCE, &[], Some(holder)),
            "id1"
        );
        assert!(
            ids.register("id1", Some(holder)),
            "the node holding id1 registers it again"
        );
        assert!(
            !ids.register("id1", Some(other)),
            "another node: docutils' Duplicate ID"
        );
        assert!(
            !ids.register("id1", None),
            "a node without identity is not the holder"
        );
        // Registering records the holder as allocating does.
        assert!(ids.register("label", Some(other)));
        assert!(ids.register("label", Some(other)));
        assert!(!ids.register("label", Some(holder)));
        // An id whose holder has no identity: no node is known to hold it,
        // so every later registration is the duplicate case.
        assert!(ids.register("plain", None));
        assert!(!ids.register("plain", None));
        assert!(!ids.register("plain", Some(holder)));
        assert!(ids.is_taken("plain"));
    }

    /// `next_uid` counts the identities minted so far, so a default state
    /// mints 1 first ([`Uid::new`] from the incremented counter).
    #[test]
    fn uids_mint_from_one() {
        let mut state = DocumentState::default();
        assert_eq!(state.mint_uid(), uid(1));
        assert_eq!(state.mint_uid(), uid(2));
        assert_eq!(state.next_uid, 2);
    }

    /// A node of `kind` with `ids`, stamped with `uid`.
    fn stamped(kind: &'static str, ids: &[&str], n: u32) -> Node {
        let mut node = Node::elem(kind, Span::ZERO);
        node.attrs.ids = strings(ids);
        node.set_uid(uid(n));
        node
    }

    /// The read-state fixture's `registries_post_parse` shape
    /// ([DS §11]): every registry as a list of `[kind, ids, attached]`
    /// entries in registration order, a dict-of-lists registry as
    /// `[key, entries]` pairs in key order, `substitution_defs` and
    /// `substitution_names` in docutils' dict order (a redefined name keeps
    /// its first position and names its last node), and `names` as
    /// `[name, id, explicit]` in `nametypes` order; the pending transforms
    /// are not part of it. A node reachable from the root is attached; one
    /// only `detached` holds is not.
    #[test]
    fn registries_dump_shape() {
        let mut state = DocumentState::default();
        for _ in 0..8 {
            state.mint_uid();
        }
        let mut para = Node::elem(kinds::PARAGRAPH, Span::ZERO);
        para.children
            .push(stamped(kinds::FOOTNOTE_REFERENCE, &["id1"], 1));
        para.children.push(stamped(kinds::REFERENCE, &[], 2));
        let mut root = Node::elem(kinds::DOCUMENT, Span::ZERO);
        root.children.push(para);
        root.children.push(stamped(kinds::TARGET, &["t"], 3));
        root.children.push(stamped("pending", &[], 5));
        root.children
            .push(stamped("substitution_definition", &[], 7));
        root.children
            .push(stamped("substitution_definition", &[], 8));
        let mut rejected = stamped("substitution_definition", &[], 4);
        rejected
            .children
            .push(stamped(kinds::FOOTNOTE_REFERENCE, &["id2"], 6));
        state.detached.push(rejected);

        let registries = &mut state.registries;
        registries.autofootnote_refs = vec![uid(1), uid(6)];
        registries.refnames.push("a", uid(3));
        registries.refnames.push("a", uid(2));
        registries.indirect_targets.push(uid(3));
        for (name, n) in [("s", 4), ("T", 7), ("s", 8)] {
            registries
                .substitution_defs
                .push((name.to_string(), uid(n)));
            registries
                .substitution_names
                .insert(name.to_lowercase(), name.to_string());
        }
        // Left out of the dump; the reachability check below still covers it.
        state.pending.push(PendingRecord {
            transform: PendingTransform::ClassAttribute,
            order: 0,
            node: uid(5),
        });
        let names = strings(&["t"]);
        let view = NodeView {
            kind: kinds::TARGET,
            names: &names,
            refuri: None,
            refname: Some("a"),
        };
        state.names.bind(view, "t", true);

        let subdef = |attached: bool| serde_json::json!(["substitution_definition", [], attached]);
        assert_eq!(
            state.registries_dump(&root),
            serde_json::json!({
                "autofootnotes": [],
                "autofootnote_refs": [
                    ["footnote_reference", ["id1"], true],
                    ["footnote_reference", ["id2"], false],
                ],
                "symbol_footnotes": [],
                "symbol_footnote_refs": [],
                "footnotes": [],
                "footnote_refs": [],
                "citations": [],
                "citation_refs": [],
                "refnames": [["a", [["target", ["t"], true], ["reference", [], true]]]],
                "indirect_targets": [["target", ["t"], true]],
                "substitution_defs": [["s", subdef(true)], ["T", subdef(true)]],
                "substitution_names": [["s", "s"], ["t", "T"]],
                "names": [["t", "t", true]],
            })
        );
        assert!(state.misplaced_registrations(&root).is_empty());
    }

    /// The end-of-parse invariant ([DS §8.2]): every registered `Uid` is
    /// reachable exactly once from the root or from `detached`. A dropped
    /// registered node is reported reached 0 times, a duplicated one by
    /// its count; the dump names an unreachable node `null`.
    #[test]
    fn misplaced_registrations_counts_each_registered_uid() {
        let mut state = DocumentState::default();
        let mut root = Node::elem(kinds::DOCUMENT, Span::ZERO);
        let once = stamped(kinds::CITATION, &["c"], 1);
        let twice = stamped(kinds::FOOTNOTE, &["id1"], 2);
        root.children.push(once);
        root.children.push(twice.clone());
        state.detached.push(twice);
        state.registries.citations.push(uid(1));
        state.registries.footnotes.push(uid(2));
        state.registries.autofootnotes.push(uid(3));
        // A uid registered twice (a `[#name]_` reference sits in three
        // registries) is still one node.
        state.registries.refnames.push("c", uid(1));
        assert_eq!(
            state.misplaced_registrations(&root),
            vec![(uid(2), 2), (uid(3), 0)]
        );
        assert_eq!(
            state.registries_dump(&root)["autofootnotes"],
            serde_json::json!([null])
        );
    }

    /// `reporter.get_source_and_line(lineno)` after the parse
    /// (`SP/docutils/statemachine.py:358-383`, `StringList.info`,
    /// `:1299-1307`): no line is the final position; a line is an offset
    /// into the top-level input, whatever source it was counted in — here
    /// `index.rst` lines 1-3, the include's padding, `inc.rst` lines 1-2
    /// and the closing padding, so line 6 is `inc.rst`'s line 2; the
    /// offset just past the end is the last line's source and its line +
    /// 1; any further one is neither; and line 0, offset -1, is Python's
    /// last item.
    #[test]
    fn source_and_line_reads_a_line_as_an_input_offset() {
        let state = DocumentState {
            final_position: DocLoc::at(3, 4),
            input_lines: vec![(0, 1), (0, 2), (0, 3), (1, 0), (2, 1), (2, 2), (3, 3)],
            ..DocumentState::default()
        };
        assert_eq!(state.source_and_line(None), DocLoc::at(3, 4));
        assert_eq!(state.source_and_line(Some(2)), DocLoc::at(0, 2));
        assert_eq!(state.source_and_line(Some(6)), DocLoc::at(2, 2));
        assert_eq!(state.source_and_line(Some(8)), DocLoc::at(3, 4));
        assert_eq!(state.source_and_line(Some(9)), DocLoc::default());
        assert_eq!(state.source_and_line(Some(0)), DocLoc::at(3, 3));
        assert_eq!(
            DocumentState::default().source_and_line(Some(1)),
            DocLoc::default()
        );
    }
}
