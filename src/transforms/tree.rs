//! The read transforms' tree: one document's nodes in an arena, with
//! parent links and node identity (wave 5a, decision 3; [DS §12.2]).
//!
//! docutils' transforms work on node objects: they walk up through
//! `node.parent`, put nodes in the place of others (`replace_self`), look
//! nodes up by id (`document.ids`) and act on registered nodes the parse
//! left out of the tree. [`TxTree`] is that object graph for one document:
//! every node of the root's tree and of the detached forest the parse kept
//! ([`DocumentState::detached`]) in one arena, each with a link to its
//! parent, addressed by [`TxId`] and, when it has an identity, by its
//! [`Uid`]. The arena never frees a node, so a node `replace_self` took out
//! of the tree, or one only the detached forest holds, stays addressable,
//! as the Python object a registry holds stays alive.
//!
//! The read builds the arena from the parse once per document
//! ([`TxTree::from_parse`]) and turns the root's tree back into a doctree
//! when the transforms are done ([`TxTree::into_doctree`]).

use std::collections::{HashMap, HashSet};

use crate::doctree::{
    is_text_element, kinds, push_astext, AstextView, AttrValue, Attrs, Doctree, Node, NodeExt,
    Span, Uid,
};
use crate::transforms::state::{DocumentState, OrderedMulti, SourceRef};

/// A node's place in the arena of the [`TxTree`] that made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TxId(u32);

impl TxId {
    fn at(index: usize) -> TxId {
        TxId(u32::try_from(index).expect("an arena holds at most u32::MAX nodes"))
    }

    fn index(self) -> usize {
        self.0 as usize
    }
}

/// One node of the arena: a [`Node`]'s data, with its children as arena
/// places and a link to its parent.
#[derive(Debug, Clone)]
pub struct TxNode {
    pub kind: &'static str,
    pub span: Span,
    pub text: Option<String>,
    pub attrs: Attrs,
    pub children: Vec<TxId>,
    /// docutils `node.parent`: the element the node was last placed in;
    /// `None` for the root, a root of the detached forest and a node never
    /// placed. As in docutils, a node taken out of its parent
    /// ([`TxTree::replace_self`]) keeps the link.
    pub parent: Option<TxId>,
    pub ext: Option<Box<NodeExt>>,
}

impl TxNode {
    /// The node's identity ([`NodeExt::uid`]).
    pub fn uid(&self) -> Option<Uid> {
        self.ext.as_deref().and_then(|ext| ext.uid)
    }

    /// docutils `node.rawsource` ([`NodeExt::rawsource`]); `None` is `''`.
    pub fn rawsource(&self) -> Option<&str> {
        self.ext.as_deref().and_then(|ext| ext.rawsource.as_deref())
    }

    /// Sets docutils' `rawsource`, as [`Node::set_rawsource`] does: the
    /// empty string, docutils' default, is stored as `None`.
    pub fn set_rawsource(&mut self, rawsource: &str) {
        if rawsource.is_empty() {
            if let Some(ext) = self.ext.as_deref_mut() {
                ext.rawsource = None;
            }
        } else {
            self.ext.get_or_insert_with(Box::default).rawsource = Some(rawsource.into());
        }
    }

    /// The scalar attribute `key`, as [`Node::get`] reads it.
    pub fn get(&self, key: &'static str) -> Option<&AttrValue> {
        self.attrs
            .extra
            .binary_search_by(|(known, _)| known.as_ref().cmp(key))
            .ok()
            .map(|at| &self.attrs.extra[at].1)
    }

    /// Sets the scalar attribute `key`, as [`Node::set`] does: `extra`
    /// stays sorted by key, and an existing value is overwritten.
    pub fn set(&mut self, key: &'static str, value: AttrValue) {
        match self
            .attrs
            .extra
            .binary_search_by(|(known, _)| known.as_ref().cmp(key))
        {
            Ok(at) => self.attrs.extra[at].1 = value,
            Err(at) => self.attrs.extra.insert(at, (key.into(), value)),
        }
    }

    /// docutils `del node[key]` for the scalar attribute `key`: its value,
    /// now removed; `None` when the node has none (where docutils raises).
    pub fn delattr(&mut self, key: &'static str) -> Option<AttrValue> {
        let at = self
            .attrs
            .extra
            .binary_search_by(|(known, _)| known.as_ref().cmp(key))
            .ok()?;
        Some(self.attrs.extra.remove(at).1)
    }

    /// The string attribute `key` (`node.get(key)` for a `str` value).
    pub fn str_attr(&self, key: &'static str) -> Option<&str> {
        match self.get(key) {
            Some(AttrValue::Str(value)) => Some(value),
            _ => None,
        }
    }
}

/// The transient attributes docutils' reference transforms keep on node
/// objects and never in a doctree: `Resolvable.resolved`
/// (`SP/docutils/nodes.py:1458`), `Targetable.referenced` (`:1477`, which
/// the parse sets on an embedded URI's target) and the
/// `expect_referenced_by_name` / `_id` maps PropagateTargets leaves on the
/// node it moved a target's ids and names to
/// (`SP/docutils/transforms/references.py:63-80`). They live beside the
/// arena, by node, for as long as the stage does ([`TxTree::resolved`],
/// [`TxTree::referenced`], [`TxTree::note_referenced_by`]).
#[derive(Debug, Default)]
struct ReferenceFlags {
    resolved: HashSet<TxId>,
    referenced: HashSet<TxId>,
    expect_by_name: HashMap<TxId, HashMap<String, TxId>>,
    expect_by_id: HashMap<TxId, HashMap<String, TxId>>,
}

/// docutils' `findall` generator as a loop over it sees it
/// (`Node._fast_findall`, `SP/docutils/nodes.py:248-253`): the node it
/// starts from and every node under it, in pre-order, each read from the
/// tree as it is when the walk gets there. The generator walks each child
/// list by index, so the walk here keeps, per node it is inside, the
/// position of the next child: a node taken out before the walk reaches it
/// is never visited, and the node that takes the place of one taken out at
/// or before the walk's position is skipped, as a Python list iterator
/// skips the element after a removed one. A transform that changes the
/// tree while it iterates, and whose loop is
/// `for node in self.document.findall(...)`, steps through one of these.
#[derive(Debug)]
pub struct Findall {
    /// Per node the walk is inside, from the start node down: the node and
    /// the index of its next child to visit.
    stack: Vec<(TxId, usize)>,
    /// The start node, until the walk has looked at it.
    start: Option<TxId>,
}

impl Findall {
    /// The walk from `from`, which it yields first when it qualifies.
    pub fn new(from: TxId) -> Findall {
        Findall {
            stack: Vec::new(),
            start: Some(from),
        }
    }

    /// The next node of the walk that `pred` accepts, if any.
    pub fn next(&mut self, tree: &TxTree, pred: impl Fn(&TxNode) -> bool) -> Option<TxId> {
        if let Some(start) = self.start.take() {
            self.stack.push((start, 0));
            if pred(tree.node(start)) {
                return Some(start);
            }
        }
        while let Some((node, next)) = self.stack.last_mut() {
            let Some(&child) = tree.node(*node).children.get(*next) else {
                self.stack.pop();
                continue;
            };
            *next += 1;
            self.stack.push((child, 0));
            if pred(tree.node(child)) {
                return Some(child);
            }
        }
        None
    }
}

/// One document's nodes in an arena (see the module docs).
#[derive(Debug)]
pub struct TxTree {
    nodes: Vec<TxNode>,
    /// The `document` node.
    root: TxId,
    /// The roots of the detached forest ([`Self::detached`]).
    detached: Vec<TxId>,
    /// The parse's source table ([`Doctree::sources`]), carried through.
    sources: Vec<String>,
    /// Every node with an identity, by it ([`Self::by_uid`]).
    by_uid: HashMap<Uid, TxId>,
    /// docutils `document.ids` ([`Self::by_id`]).
    ids: HashMap<String, TxId>,
    /// docutils `document.refids` ([`Self::refids_mut`]).
    refids: OrderedMulti<TxId>,
    /// The reference transforms' transient node attributes.
    flags: ReferenceFlags,
}

impl TxTree {
    /// The arena of a parsed document: the doctree's root and the detached
    /// forest the parse kept, which it takes out of `state` (decision 2).
    /// Nodes are placed in pre-order, the root's tree first and then each
    /// detached subtree. Every node with an identity is addressable by it,
    /// and the ids map is docutils' `document.ids` as the parse leaves it
    /// ([DS §4.4 (4), §8.1]): each id names its first carrier in that
    /// order, which may be a detached node. One gap: an id the parse
    /// registered on a node in a subtree it discarded without keeping it
    /// (`DocumentState::detach`) names that node in `document.ids` but
    /// nothing here ([`Self::by_id`]).
    pub fn from_parse(doctree: Doctree, state: &mut DocumentState) -> TxTree {
        let Doctree { root, sources } = doctree;
        let mut tree = TxTree {
            nodes: Vec::new(),
            root: TxId(0),
            detached: Vec::new(),
            sources,
            by_uid: HashMap::new(),
            ids: HashMap::new(),
            refids: OrderedMulti::default(),
            flags: ReferenceFlags::default(),
        };
        tree.root = tree.place(root, None);
        for subtree in std::mem::take(&mut state.detached) {
            let id = tree.place(subtree, None);
            tree.detached.push(id);
        }
        for (index, node) in tree.nodes.iter().enumerate() {
            let id = TxId::at(index);
            if let Some(uid) = node.uid() {
                tree.by_uid.entry(uid).or_insert(id);
            }
            for key in &node.attrs.ids {
                tree.ids.entry(key.clone()).or_insert(id);
            }
            // The parse marks an embedded URI's target referenced
            // (`target.referenced = 1`, `SP/docutils/parsers/rst/states.py:942`):
            // the one target in a TextElement that has a `refuri` when the
            // parse ends.
            let embedded_uri = node.kind == kinds::TARGET
                && node.get("refuri").is_some()
                && node
                    .parent
                    .is_some_and(|parent| is_text_element(tree.nodes[parent.index()].kind));
            if embedded_uri {
                tree.flags.referenced.insert(id);
            }
        }
        tree
    }

    /// Places `node` and its subtree in the arena, in pre-order, linked to
    /// `parent`.
    fn place(&mut self, node: Node, parent: Option<TxId>) -> TxId {
        let Node {
            kind,
            span,
            text,
            attrs,
            children,
            ext,
        } = node;
        let id = self.push(TxNode {
            kind,
            span,
            text,
            attrs,
            children: Vec::with_capacity(children.len()),
            parent,
            ext,
        });
        for child in children {
            let child = self.place(child, Some(id));
            self.nodes[id.index()].children.push(child);
        }
        id
    }

    fn push(&mut self, node: TxNode) -> TxId {
        let id = TxId::at(self.nodes.len());
        self.nodes.push(node);
        id
    }

    /// The doctree of the root's tree as it stands, with the parse's
    /// source table. The detached forest and every node out of the tree
    /// are left behind.
    pub fn into_doctree(self) -> Doctree {
        let TxTree {
            nodes,
            root,
            sources,
            ..
        } = self;
        let mut nodes: Vec<Option<TxNode>> = nodes.into_iter().map(Some).collect();
        let root = take_subtree(&mut nodes, root).expect("the root is placed once");
        Doctree { root, sources }
    }

    /// The `document` node.
    pub fn root(&self) -> TxId {
        self.root
    }

    /// The roots of the detached forest: the subtrees the parse discarded
    /// that hold a registered or pending node, in the order it discarded
    /// them.
    pub fn detached(&self) -> &[TxId] {
        &self.detached
    }

    /// The node at `id`.
    pub fn node(&self, id: TxId) -> &TxNode {
        &self.nodes[id.index()]
    }

    /// The node at `id`, to change. Its `children` and `parent` are the
    /// links [`Self::append`] and [`Self::replace_self`] keep in step; and
    /// as in docutils, changing its `ids` changes no ids-map entry
    /// ([`Self::note_id`]).
    pub fn node_mut(&mut self, id: TxId) -> &mut TxNode {
        &mut self.nodes[id.index()]
    }

    /// The node carrying `uid`, wherever it is: in the tree, in the
    /// detached forest, out of the tree since a [`Self::replace_self`], or
    /// a copy [`Self::deepcopy`] made.
    pub fn by_uid(&self, uid: Uid) -> Option<TxId> {
        self.by_uid.get(&uid).copied()
    }

    /// docutils `document.ids.get(id)`: the node the id was registered
    /// for, which need not be in the tree. A detached node's id names it,
    /// and a node replaced since still holds the ids it had. Unlike
    /// docutils' map it has no node for an id the parse registered on a
    /// node it then dropped (a `:name:`d image in a rejected table, say,
    /// [`Self::from_parse`]): `None` there, where docutils' lookup finds
    /// the dropped node, and each caller handles it so that the read comes
    /// out as Sphinx's (a reference and an alias to that name resolve with
    /// no record, probed on Sphinx 9.1.0).
    pub fn by_id(&self, id: &str) -> Option<TxId> {
        self.ids.get(id).copied()
    }

    /// docutils `document.ids[id] = node`, for what changes docutils' map:
    /// `set_id` registering a node (`note_implicit_target` through it) and
    /// PropagateTargets moving a target's ids to the next node
    /// (`references.py:69`). [`Self::replace_self`] hands ids on to a
    /// replacement and [`Self::deepcopy`] copies them, but neither changes
    /// the map.
    pub fn note_id(&mut self, id: &str, node: TxId) {
        self.ids.insert(id.to_string(), node);
    }

    /// docutils `document.refids`: the nodes noted under the id they refer
    /// to (`note_refid`). The parse notes none, so it starts empty
    /// ([DS §4.5]).
    pub fn refids_mut(&mut self) -> &mut OrderedMulti<TxId> {
        &mut self.refids
    }

    /// docutils `document.refids`, to read ([`Self::refids_mut`]).
    pub fn refids(&self) -> &OrderedMulti<TxId> {
        &self.refids
    }

    /// docutils `node.resolved`: whether a transform has resolved the
    /// reference or indirect target at `id` (or given up on it). A copy
    /// ([`Self::deepcopy`]) starts unresolved, as a new node object does.
    pub fn resolved(&self, id: TxId) -> bool {
        self.flags.resolved.contains(&id)
    }

    /// `node.resolved = True`.
    pub fn set_resolved(&mut self, id: TxId) {
        self.flags.resolved.insert(id);
    }

    /// docutils `node.referenced`: whether a reference has been resolved
    /// to the node at `id`, the parse marked it (an embedded URI's target,
    /// [`Self::from_parse`]), or `dupname()` moved one of its names to
    /// `dupnames`, which marks the node referenced "even though it isn't"
    /// (`SP/docutils/nodes.py:3036-3041`) and is all that fills `dupnames`.
    /// Only `DanglingReferences`' INFO about targets nothing referenced
    /// reads it.
    pub fn referenced(&self, id: TxId) -> bool {
        self.flags.referenced.contains(&id) || !self.node(id).attrs.dupnames.is_empty()
    }

    /// `node.referenced = True`.
    pub fn set_referenced(&mut self, id: TxId) {
        self.flags.referenced.insert(id);
    }

    /// docutils `node.note_referenced_by(name, id)` (`nodes.py:1202-1220`):
    /// the node at `id` is referenced, and so is the target whose `name`
    /// or `ref_id` PropagateTargets moved to it
    /// ([`Self::expect_referenced_by`]).
    pub fn note_referenced_by(&mut self, id: TxId, name: Option<&str>, ref_id: Option<&str>) {
        self.flags.referenced.insert(id);
        let by_name = name.and_then(|name| self.flags.expect_by_name.get(&id)?.get(name));
        let by_id = ref_id.and_then(|key| self.flags.expect_by_id.get(&id)?.get(key));
        let expected: Vec<TxId> = by_name.into_iter().chain(by_id).copied().collect();
        self.flags.referenced.extend(expected);
    }

    /// PropagateTargets' bookkeeping for the target at `target`, whose
    /// `ids` and `names` it moved to the node at `receiver`
    /// (`references.py:62-80`): being referenced by one of them marks the
    /// target referenced, and so does whatever marked the target itself.
    pub fn expect_referenced_by(
        &mut self,
        receiver: TxId,
        target: TxId,
        ids: &[String],
        names: &[String],
    ) {
        let inherited_ids = self.flags.expect_by_id.get(&target).cloned();
        let inherited_names = self.flags.expect_by_name.get(&target).cloned();
        let by_id = self.flags.expect_by_id.entry(receiver).or_default();
        for id in ids {
            by_id.insert(id.clone(), target);
        }
        by_id.extend(inherited_ids.unwrap_or_default());
        let by_name = self.flags.expect_by_name.entry(receiver).or_default();
        for name in names {
            by_name.insert(name.clone(), target);
        }
        by_name.extend(inherited_names.unwrap_or_default());
    }

    /// docutils `node.parent` ([`TxNode::parent`]).
    pub fn parent(&self, id: TxId) -> Option<TxId> {
        self.node(id).parent
    }

    /// docutils `list(node.findall(condition))`: the node at `from` and
    /// every node under it, Text nodes included, in pre-order, keeping
    /// those `pred` accepts. A snapshot, as the list is: changing the tree
    /// while working through it changes nothing it holds.
    pub fn findall(&self, from: TxId, pred: impl Fn(&TxNode) -> bool) -> Vec<TxId> {
        let mut found = Vec::new();
        let mut stack = vec![from];
        while let Some(id) = stack.pop() {
            let node = self.node(id);
            if pred(node) {
                found.push(id);
            }
            stack.extend(node.children.iter().rev());
        }
        found
    }

    /// docutils `node.next_node(condition, include_self, descend,
    /// siblings, ascend)`: the first node `pred` accepts in `findall`'s
    /// walk with those flags, read from the tree as it is now. The walk is
    /// the node itself (`include_self`), its descendants in pre-order
    /// (`descend`), then each following sibling, with its subtree when
    /// `descend` (`siblings`), then the same for each ancestor in turn,
    /// never the ancestor itself (`ascend`, which implies `siblings`).
    pub fn next_node(
        &self,
        id: TxId,
        pred: impl Fn(&TxNode) -> bool,
        include_self: bool,
        descend: bool,
        siblings: bool,
        ascend: bool,
    ) -> Option<TxId> {
        if include_self && pred(self.node(id)) {
            return Some(id);
        }
        if descend {
            let children = &self.node(id).children;
            let below = children
                .iter()
                .find_map(|&child| self.first_at_or_below(child, &pred));
            if below.is_some() {
                return below;
            }
        }
        if !(siblings || ascend) {
            return None;
        }
        let mut node = id;
        while let Some(parent) = self.parent(node) {
            let children = &self.node(parent).children;
            let Some(index) = children.iter().position(|&child| child == node) else {
                // docutils' `parent.index(node)` raises: a node replaced
                // since has no siblings to walk. The message names no node:
                // ClassAttribute starts the walk from a node it found by
                // its identity, and CodeQL's rust/cleartext-logging reads
                // anything that flows from a `uid` as sensitive.
                debug_assert!(
                    false,
                    "next_node: the node is not among its parent's children"
                );
                return None;
            };
            for &sibling in &children[index + 1..] {
                let found = if descend {
                    self.first_at_or_below(sibling, &pred)
                } else {
                    pred(self.node(sibling)).then_some(sibling)
                };
                if found.is_some() {
                    return found;
                }
            }
            if !ascend {
                break;
            }
            node = parent;
        }
        None
    }

    /// The first node `pred` accepts at or under `id`, in pre-order.
    fn first_at_or_below(&self, id: TxId, pred: &impl Fn(&TxNode) -> bool) -> Option<TxId> {
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            let node = self.node(id);
            if pred(node) {
                return Some(id);
            }
            stack.extend(node.children.iter().rev());
        }
        None
    }

    /// docutils `node.replace_self(new)` for a list `new`
    /// (`nodes.py:1110-1132`): the first replacement, when it is an
    /// element, takes the replaced node's `ids`, `classes`, `names` and
    /// `dupnames` it does not hold yet, after its own
    /// (`update_basic_atts`; [DS §5.5]: a problematic with an id of its own
    /// reads `ids="<new> <old>"`). The replacements then take the node's
    /// place among its parent's children, in order, and link to the
    /// parent. The ids map is untouched, so the node's ids still name it
    /// ([`Self::by_id`]); the node keeps its parent link and its place in
    /// the arena. A root of the detached forest is replaced among the
    /// forest's roots, as docutils replaces it in the throwaway element a
    /// directive parsed it into.
    pub fn replace_self(&mut self, id: TxId, with: Vec<TxId>) {
        match with.first() {
            Some(&first) if self.node(first).kind != kinds::TEXT => {
                let old = &self.node(id).attrs;
                let basic = [
                    old.ids.clone(),
                    old.classes.clone(),
                    old.names.clone(),
                    old.dupnames.clone(),
                ];
                let new = &mut self.node_mut(first).attrs;
                let lists = [
                    &mut new.ids,
                    &mut new.classes,
                    &mut new.names,
                    &mut new.dupnames,
                ];
                for (list, values) in lists.into_iter().zip(basic) {
                    for value in values {
                        if !list.contains(&value) {
                            list.push(value);
                        }
                    }
                }
            }
            _ => {
                // docutils asserts that no attribute is lost.
                let old = &self.node(id).attrs;
                debug_assert!(
                    old.ids.is_empty()
                        && old.classes.is_empty()
                        && old.names.is_empty()
                        && old.dupnames.is_empty(),
                    "replacing a {} with text or nothing loses its attributes",
                    self.node(id).kind
                );
            }
        }
        let parent = self.parent(id);
        let siblings = match parent {
            Some(parent) => &mut self.nodes[parent.index()].children,
            None => &mut self.detached,
        };
        let Some(index) = siblings.iter().position(|&node| node == id) else {
            // The message names no node: CodeQL's rust/cleartext-logging
            // reads anything that flows from a `uid` as sensitive.
            debug_assert!(
                false,
                "replace_self: the node has no place to be replaced in"
            );
            return;
        };
        siblings.splice(index..=index, with.iter().copied());
        for node in with {
            self.nodes[node.index()].parent = parent;
        }
    }

    /// A new element of `kind`, as `nodes.<kind>()` makes it: in no
    /// parent, with no attributes, no children and no location (a node a
    /// transform makes has none, docutils' current line being unset). A
    /// Text node takes its `text` afterwards.
    pub fn new_node(&mut self, kind: &'static str) -> TxId {
        self.push(TxNode {
            kind,
            span: Span::ZERO,
            text: None,
            attrs: Attrs::default(),
            children: Vec::new(),
            parent: None,
            ext: None,
        })
    }

    /// The owned subtree `node`, placed in the arena in no parent, as a
    /// node a transform builds whole is (a reporter's `system_message`).
    /// Its nodes have no identity and register no id.
    pub fn new_subtree(&mut self, node: Node) -> TxId {
        self.place(node, None)
    }

    /// docutils `parent.append(child)` (`parent += child`): `child` goes
    /// last among `parent`'s children and links to it.
    pub fn append(&mut self, parent: TxId, child: TxId) {
        self.node_mut(child).parent = Some(parent);
        self.node_mut(parent).children.push(child);
    }

    /// docutils `parent.insert(index, child)` for a node: `child` goes in
    /// at `index` among `parent`'s children and links to it.
    pub fn insert(&mut self, parent: TxId, index: usize, child: TxId) {
        self.node_mut(child).parent = Some(parent);
        self.node_mut(parent).children.insert(index, child);
    }

    /// docutils `parent.remove(child)`: `child` leaves `parent`'s
    /// children, keeping its parent link, as in docutils. Removing a node
    /// that is not among them, which raises in Python, does nothing.
    pub fn remove(&mut self, parent: TxId, child: TxId) {
        let children = &mut self.node_mut(parent).children;
        match children.iter().position(|&node| node == child) {
            Some(index) => {
                children.remove(index);
            }
            None => debug_assert!(false, "remove: the node is not among the children"),
        }
    }

    /// The parse's source table ([`Doctree::sources`]).
    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// The path of `source` in the parse's source table; for a location
    /// that names no source, the document's own, the table's first entry
    /// (docutils' `reporter.source`), as the parse's reporter spells it.
    pub fn source_path(&self, source: Option<SourceRef>) -> &str {
        let index = source.map_or(0, |SourceRef::Table(index)| usize::from(index));
        self.sources
            .get(index)
            .or_else(|| self.sources.first())
            .map_or("<string>", String::as_str)
    }

    /// The identity of the node at `id`, minted from `state` when it has
    /// none ([`DocumentState::mint_uid`]): for a node a transform
    /// registers (TargetNotes' footnotes and references) or, with the id
    /// trace on, the node a traced id names. The node is then addressable
    /// by it ([`Self::by_uid`]).
    pub fn assign_uid(&mut self, id: TxId, state: &mut DocumentState) -> Uid {
        if let Some(uid) = self.node(id).uid() {
            return uid;
        }
        let uid = state.mint_uid();
        self.set_uid(id, uid);
        uid
    }

    fn set_uid(&mut self, id: TxId, uid: Uid) {
        self.node_mut(id).ext.get_or_insert_with(Box::default).uid = Some(uid);
        self.by_uid.insert(uid, id);
    }

    /// Python's `node.deepcopy()`: a copy of the subtree at `id`, in no
    /// parent. Each node keeps its kind, span, text and attributes, its
    /// ids verbatim (no id is allocated), and its side-car's `rawsource`,
    /// Text data and pending transform; `Element.copy` takes `source` and
    /// `line` along, so a copied node's location override is the
    /// original's. Only a node that had an identity gets one, minted from
    /// `state` in pre-order, which [`Self::by_uid`] resolves to the copy.
    /// Neither a registry nor the ids map learns of the copies; a
    /// transform that needs them noted notes them (Substitutions'
    /// `note_refname`).
    pub fn deepcopy(&mut self, id: TxId, state: &mut DocumentState) -> TxId {
        let original = self.node(id);
        let identity = original.uid();
        let children = original.children.clone();
        let copy = TxNode {
            kind: original.kind,
            span: original.span,
            text: original.text.clone(),
            attrs: original.attrs.clone(),
            children: Vec::with_capacity(children.len()),
            parent: None,
            ext: original.ext.as_deref().map(|ext| {
                Box::new(NodeExt {
                    uid: None,
                    ..ext.clone()
                })
            }),
        };
        let copy = self.push(copy);
        if let Some(original) = identity {
            let uid = state.mint_uid();
            self.set_uid(copy, uid);
            if let Some(&loc) = state.loc_overrides.get(&original) {
                state.loc_overrides.insert(uid, loc);
            }
        }
        for child in children {
            let child = self.deepcopy(child, state);
            self.append(copy, child);
        }
        copy
    }

    /// docutils `node.astext()` of the subtree at `id`
    /// ([`Node::astext_docutils`]), read in place.
    pub fn astext(&self, id: TxId) -> String {
        let mut text = String::new();
        push_astext(TxView { tree: self, id }, &mut text);
        text
    }
}

/// A node of the arena as [`push_astext`] reads it.
#[derive(Clone, Copy)]
struct TxView<'a> {
    tree: &'a TxTree,
    id: TxId,
}

impl<'a> AstextView<'a> for TxView<'a> {
    fn kind(self) -> &'static str {
        self.tree.node(self.id).kind
    }

    fn text(self) -> Option<&'a str> {
        self.tree.node(self.id).text.as_deref()
    }

    fn attr(self, key: &'static str) -> Option<&'a AttrValue> {
        self.tree.node(self.id).get(key)
    }

    fn children(self) -> impl Iterator<Item = Self> {
        let tree = self.tree;
        tree.node(self.id)
            .children
            .iter()
            .map(move |&id| TxView { tree, id })
    }
}

/// The owned subtree at `id`, moved out of `nodes`. A node already moved
/// out is one placed under two parents: it is left out the second time.
fn take_subtree(nodes: &mut [Option<TxNode>], id: TxId) -> Option<Node> {
    let Some(TxNode {
        kind,
        span,
        text,
        attrs,
        children,
        parent: _,
        ext,
    }) = nodes[id.index()].take()
    else {
        debug_assert!(false, "{id:?} is placed twice in the tree");
        return None;
    };
    let children = children
        .into_iter()
        .filter_map(|child| take_subtree(nodes, child))
        .collect();
    Some(Node {
        kind,
        span,
        text,
        attrs,
        children,
        ext,
    })
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;
    use crate::doctree::{kinds, Node};
    use crate::rst::tests::for_each_corpus_parse;
    use crate::rst::{parse_rst_full, ParseOptions, ParseOutput};

    fn uid(n: u32) -> Uid {
        Uid::new(NonZeroU32::new(n).expect("a non-zero uid"))
    }

    /// A snippet parsed as the read-state differential parses it.
    fn read_parse(source: &str) -> ParseOutput {
        parse_rst_full(
            source,
            &ParseOptions {
                source_path: "<SRCDIR>/index.rst".to_string(),
                sphinx: true,
                docname: "index".to_string(),
                ..ParseOptions::default()
            },
        )
    }

    /// The side-car data of every node in pre-order that `Node`'s
    /// equality leaves out: its identity and its Text data.
    fn read_stage_data(node: &Node, out: &mut Vec<(Option<Uid>, Option<String>)>) {
        let raw = node
            .ext
            .as_deref()
            .and_then(|ext| ext.raw.as_deref())
            .map(str::to_string);
        out.push((node.uid(), raw));
        for child in &node.children {
            read_stage_data(child, out);
        }
    }

    /// [DS §12.5] step 2: a parse turned into the arena and back is the
    /// parse, on every input of the four parse corpora: the same pformat,
    /// an equal `Doctree` (so `rawsource`, `pending` and the source table
    /// survive) and the same identities and Text data. Between the two,
    /// every registered and pending identity resolves through `by_uid`,
    /// the detached forest's included, to the node carrying it, and the
    /// arena's `astext` is the doctree's.
    #[test]
    fn txtree_round_trip_is_identity() {
        let mut parses = [0usize; 4];
        let mut detached_roots = 0;
        for_each_corpus_parse(|corpus, label, output| {
            let ParseOutput {
                doctree, mut state, ..
            } = output;
            let expected = doctree.clone();
            let mut expected_data = Vec::new();
            read_stage_data(&expected.root, &mut expected_data);
            let held: Vec<Uid> = state.held_uids().collect();
            detached_roots += state.detached.len();

            let tree = TxTree::from_parse(doctree, &mut state);
            assert!(
                state.detached.is_empty(),
                "{label}: the arena takes the forest"
            );
            for uid in held {
                let id = tree
                    .by_uid(uid)
                    .unwrap_or_else(|| panic!("{label}: a held identity does not resolve"));
                assert_eq!(tree.node(id).uid(), Some(uid), "{label}");
            }
            assert_eq!(
                tree.astext(tree.root()),
                expected.root.astext_docutils(),
                "{label}"
            );

            let back = tree.into_doctree();
            assert_eq!(back.root.pformat(), expected.root.pformat(), "{label}");
            assert!(
                back == expected,
                "{label}: the round trip changed the doctree"
            );
            let mut back_data = Vec::new();
            read_stage_data(&back.root, &mut back_data);
            assert_eq!(back_data, expected_data, "{label}");
            parses[corpus as usize] += 1;
        });
        assert!(
            parses[0] >= 2 * 735 && parses[1] >= 493 && parses[2] >= 89 && parses[3] >= 99,
            "too few parses: {parses:?}"
        );
        // The docutils corpus's rejected substitution definition (both
        // modes) and the read-state corpus's rejected substitution
        // definitions and tables.
        assert!(detached_roots >= 2 + 5, "{detached_roots} detached roots");
    }

    /// Whether `node`'s parent links lead to the root.
    fn under_root(tree: &TxTree, mut node: TxId) -> bool {
        while let Some(parent) = tree.parent(node) {
            node = parent;
        }
        node == tree.root()
    }

    /// A hand-built node of `kind` carrying `ids`.
    fn with_ids(kind: &'static str, ids: &[&str], children: Vec<Node>) -> Node {
        let mut node = Node::elem(kind, Span::ZERO);
        node.attrs.ids = ids.iter().map(|id| id.to_string()).collect();
        node.children = children;
        node
    }

    /// docutils `document.ids` ([DS §4.4 (4), §8.1]). `from_parse` maps
    /// each id to its first carrier in a pre-order walk of the root's tree
    /// and then of the detached forest, so an id may name a node outside
    /// the tree: the rejected table's references of
    /// `state.rejected_table_refs` keep `id1` and `id2` there. Only
    /// `note_id` changes the map (`set_id`, PropagateTargets' overwrite):
    /// a `replace_self` that hands a node's ids on to its replacement
    /// leaves them naming the replaced node.
    #[test]
    fn ids_map_is_first_registrant_and_ignores_replace_self() {
        let ParseOutput {
            doctree, mut state, ..
        } = read_parse(
            ".. table:: Cap [#]_\n\n   not a table [#]_\n\nText [#]_.\n\n.. [#] a\n.. [#] b\n",
        );
        let tree = TxTree::from_parse(doctree, &mut state);
        for (id, kind, attached) in [
            ("id1", kinds::FOOTNOTE_REFERENCE, false),
            ("id2", kinds::FOOTNOTE_REFERENCE, false),
            ("id3", kinds::FOOTNOTE_REFERENCE, true),
            ("id4", kinds::FOOTNOTE, true),
            ("id5", kinds::FOOTNOTE, true),
        ] {
            let node = tree.by_id(id).unwrap_or_else(|| panic!("{id} is mapped"));
            assert_eq!(tree.node(node).kind, kind, "{id}");
            assert_eq!(tree.node(node).attrs.ids, [id]);
            assert_eq!(under_root(&tree, node), attached, "{id}");
        }
        assert_eq!(tree.by_id("id6"), None);

        // Duplicate ids: the root's tree in pre-order, then the forest.
        let root = with_ids(
            kinds::DOCUMENT,
            &[],
            vec![
                with_ids(kinds::PARAGRAPH, &["a"], Vec::new()),
                with_ids(
                    kinds::SECTION,
                    &["a", "b"],
                    vec![with_ids(kinds::TARGET, &["b", "c"], Vec::new())],
                ),
            ],
        );
        let mut state = DocumentState::default();
        state
            .detached
            .push(with_ids(kinds::TARGET, &["c", "d"], Vec::new()));
        let doctree = Doctree {
            root,
            sources: vec!["index.rst".to_string()],
        };
        let mut tree = TxTree::from_parse(doctree, &mut state);
        let (paragraph, section) = match tree.node(tree.root()).children[..] {
            [paragraph, section] => (paragraph, section),
            ref other => panic!("two children, got {other:?}"),
        };
        let target = tree.node(section).children[0];
        let [forest] = tree.detached() else {
            panic!("one detached root");
        };
        let forest = *forest;
        assert_eq!(tree.by_id("a"), Some(paragraph));
        assert_eq!(tree.by_id("b"), Some(section));
        assert_eq!(tree.by_id("c"), Some(target));
        assert_eq!(tree.by_id("d"), Some(forest));
        assert_eq!(tree.by_id("e"), None);

        let replacement = tree.new_node(kinds::PROBLEMATIC);
        tree.node_mut(replacement).attrs.ids.push("n".to_string());
        tree.replace_self(paragraph, vec![replacement]);
        assert_eq!(tree.node(replacement).attrs.ids, ["n", "a"]);
        assert_eq!(tree.by_id("a"), Some(paragraph), "replace_self moves no id");
        assert_eq!(tree.by_id("n"), None, "nothing noted n");
        tree.note_id("n", replacement);
        assert_eq!(tree.by_id("n"), Some(replacement));
        // PropagateTargets: `document.ids[id] = next_node`.
        tree.note_id("a", section);
        assert_eq!(tree.by_id("a"), Some(section));
        assert_eq!(tree.refids_mut().iter().count(), 0, "refids starts empty");
        tree.refids_mut().push("a", target);
        assert_eq!(tree.refids_mut().get("a"), Some(&[target][..]));
    }

    /// Python's `Element.deepcopy` (220 copies a substitution definition's
    /// content this way): every node is copied with its attributes, its
    /// ids verbatim, and its side-car (`rawsource`, a Text's escaped data,
    /// a `pending` node's transform), and `Element.copy` takes `source` and
    /// `line` along, so a copied node with a location override gets the
    /// original's under its new identity. Only nodes that had an identity
    /// get one, minted in pre-order. No id is allocated, and neither the
    /// registries nor the ids map learn of the copies.
    #[test]
    fn deepcopy_clones_side_car_and_location_but_registers_nothing() {
        // docutils mode: `.. class::` leaves a `pending` node.
        let source = ".. class:: special\n\n\
                      Escaped \\*text\\* with a ref_ and `alias <target_>`_.\n\n\
                      .. image:: pic.png\n   :target: https://x/\n\n\
                      .. note::\n\n   .. image:: pic.png\n      :target: https://y/\n";
        let ParseOutput {
            doctree, mut state, ..
        } = parse_rst_full(
            source,
            &ParseOptions {
                source_path: "<snippet>".to_string(),
                ..ParseOptions::default()
            },
        );
        let mut tree = TxTree::from_parse(doctree, &mut state);
        let root = tree.root();
        let held: Vec<Uid> = state.held_uids().collect();
        let overrides = state.loc_overrides.clone();
        let next_id = state.ids.clone().allocate(kinds::PROBLEMATIC, &[], None);
        let minted = state.next_uid;

        let copy = tree.deepcopy(root, &mut state);

        assert_eq!(tree.parent(copy), None);
        let originals = tree.findall(root, |_| true);
        let copies = tree.findall(copy, |_| true);
        assert_eq!(originals.len(), copies.len());
        let side_car = |node: &TxNode| {
            node.ext
                .as_deref()
                .map(|ext| (ext.rawsource.clone(), ext.raw.clone(), ext.pending.clone()))
                .filter(|data| *data != (None, None, None))
        };
        let mut fresh = Vec::new();
        for (&original, &copied) in originals.iter().zip(&copies) {
            assert_ne!(original, copied);
            let (a, b) = (tree.node(original), tree.node(copied));
            assert_eq!(
                (a.kind, a.span, &a.text, &a.attrs),
                (b.kind, b.span, &b.text, &b.attrs)
            );
            assert_eq!(side_car(a), side_car(b), "{}", a.kind);
            if original != root {
                let parent = tree.parent(copied).expect("a copied child has a parent");
                let position = copies.iter().position(|&node| node == parent);
                let expected = originals
                    .iter()
                    .position(|&node| Some(node) == tree.parent(original));
                assert_eq!(position, expected, "the copy keeps the shape");
            }
            match (a.uid(), b.uid()) {
                (None, None) => {}
                (Some(old), Some(new)) => {
                    assert_ne!(old, new);
                    assert_eq!(tree.by_uid(new), Some(copied));
                    assert_eq!(state.loc_overrides.get(&new), overrides.get(&old));
                    fresh.push(new);
                }
                (None, Some(_)) => {
                    panic!("{}: the copy has an identity its original lacks", a.kind)
                }
                (Some(_), None) => panic!("{}: the copy lost its original's identity", a.kind),
            }
        }
        // The pending node, `ref_`, the alias's reference and target, and
        // the two image references, each with a new identity in turn.
        assert_eq!(
            fresh,
            (minted + 1..=state.next_uid).map(uid).collect::<Vec<_>>()
        );
        assert_eq!(fresh.len(), 6);
        assert_eq!(state.loc_overrides.len(), 2 * overrides.len());
        assert!(originals
            .iter()
            .any(|&node| side_car(tree.node(node)).is_some_and(|data| data.1.is_some())));
        assert!(originals
            .iter()
            .any(|&node| side_car(tree.node(node)).is_some_and(|data| data.2.is_some())));

        assert_eq!(state.held_uids().collect::<Vec<_>>(), held);
        for &copied in &copies {
            for id in &tree.node(copied).attrs.ids {
                assert_ne!(tree.by_id(id), Some(copied), "{id}");
                assert!(tree.by_id(id).is_some(), "{id}");
            }
        }
        assert_eq!(
            state.ids.clone().allocate(kinds::PROBLEMATIC, &[], None),
            next_id
        );
        assert_eq!(tree.astext(copy), tree.astext(root));
    }

    /// `problematic(rawsource, rawsource, refid=msgid)`.
    fn problematic(tree: &mut TxTree, rawsource: &str, refid: &str) -> TxId {
        let prb = tree.new_node(kinds::PROBLEMATIC);
        tree.node_mut(prb).attrs.extra.push((
            "refid".into(),
            crate::doctree::AttrValue::Str(refid.to_string()),
        ));
        tree.node_mut(prb).ext = Some(Box::new(NodeExt {
            rawsource: Some(rawsource.into()),
            ..NodeExt::default()
        }));
        let text = tree.new_node(kinds::TEXT);
        tree.node_mut(text).text = Some(rawsource.to_string());
        tree.append(prb, text);
        prb
    }

    /// One failed reference: a loose message takes the next id
    /// (`set_id(msg)`), then a problematic replaces the reference, with an
    /// id of its own (`set_id(prb)`: the allocating shape of 220, 440, 460
    /// and 620) or with none (850, when the reference has ids: `prbid =
    /// node['ids'][0]`).
    fn fail(tree: &mut TxTree, state: &mut DocumentState, reference: TxId, allocate: bool) {
        let message = tree.new_node(kinds::SYSTEM_MESSAGE);
        let msgid = state.ids.allocate(kinds::SYSTEM_MESSAGE, &[], None);
        tree.node_mut(message).attrs.ids.push(msgid.clone());
        tree.note_id(&msgid, message);
        let rawsource = tree
            .node(reference)
            .rawsource()
            .expect("a Referential keeps its rawsource")
            .to_string();
        let prb = problematic(tree, &rawsource, &msgid);
        if allocate {
            let prbid = state.ids.allocate(kinds::PROBLEMATIC, &[], None);
            tree.node_mut(prb).attrs.ids.push(prbid.clone());
            tree.note_id(&prbid, prb);
        }
        tree.replace_self(reference, vec![prb]);
    }

    /// docutils `replace_self` ([DS §5.5]): `update_basic_atts` appends
    /// the replaced node's `ids`, `classes`, `names` and `dupnames` to the
    /// first replacement's, skipping values it holds, so a problematic
    /// with an id of its own reads `ids="<new> <old>"` and one without
    /// takes the reference's. The first half replays 620 and 850 on
    /// `docutils-0236` (`state.problematic_id_shapes`), whose post-read
    /// tree is the read-state fixture's, translation progress aside. The
    /// replaced references leave the tree, keep their parent link (docutils
    /// never clears it) and stay addressable.
    #[test]
    fn replace_self_merges_ids() {
        let ParseOutput {
            doctree, mut state, ..
        } = read_parse("Refs [1]_ [#]_ [#label]_ [*]_ end.\n");
        let mut tree = TxTree::from_parse(doctree, &mut state);
        let registered = |uid: Uid| tree.by_uid(uid).expect("registered");
        let auto = registered(state.registries.autofootnote_refs[0]);
        let symbol = registered(state.registries.symbol_footnote_refs[0]);
        let numbered = registered(state.registries.footnote_refs.get("1").expect("[1]_")[0]);
        let labeled = registered(
            state
                .registries
                .footnote_refs
                .get("label")
                .expect("[#label]_")[0],
        );
        let paragraph = tree.parent(auto).expect("in a paragraph");
        // 620: `[#]_` has no footnote (`[#label]_` has a refname and is
        // skipped), nor has `[*]_`.
        fail(&mut tree, &mut state, auto, true);
        fail(&mut tree, &mut state, symbol, true);
        // 850: the unknown names, references with ids.
        fail(&mut tree, &mut state, numbered, false);
        fail(&mut tree, &mut state, labeled, false);
        for reference in [auto, symbol, numbered, labeled] {
            assert_eq!(tree.parent(reference), Some(paragraph));
            assert!(!tree.node(paragraph).children.contains(&reference));
            assert_eq!(
                tree.by_uid(tree.node(reference).uid().expect("an identity")),
                Some(reference)
            );
        }
        assert_eq!(tree.by_id("id2"), Some(auto));
        assert_eq!(
            tree.by_id("id6").map(|prb| tree.node(prb).kind),
            Some(kinds::PROBLEMATIC)
        );
        assert_eq!(
            tree.into_doctree().root.pformat(),
            concat!(
                "<document source=\"<SRCDIR>/index.rst\">\n",
                "    <paragraph>\n",
                "        Refs \n",
                "        <problematic ids=\"id1\" refid=\"id9\">\n",
                "            [1]_\n",
                "         \n",
                "        <problematic ids=\"id6 id2\" refid=\"id5\">\n",
                "            [#]_\n",
                "         \n",
                "        <problematic ids=\"id3\" refid=\"id10\">\n",
                "            [#label]_\n",
                "         \n",
                "        <problematic ids=\"id8 id4\" refid=\"id7\">\n",
                "            [*]_\n",
                "         end.\n",
            )
        );

        // All four basic attributes, never `backrefs`; values already
        // there stay where they are; only the first replacement takes
        // them, and every replacement joins the parent in order.
        let mut old = with_ids(kinds::FOOTNOTE_REFERENCE, &["a", "b"], Vec::new());
        old.attrs.classes = vec!["c".to_string()];
        old.attrs.names = vec!["n".to_string()];
        old.attrs.dupnames = vec!["d".to_string()];
        old.attrs.backrefs = vec!["x".to_string()];
        let root = with_ids(
            kinds::DOCUMENT,
            &[],
            vec![with_ids(kinds::PARAGRAPH, &[], vec![old])],
        );
        let mut state = DocumentState::default();
        let mut tree = TxTree::from_parse(
            Doctree {
                root,
                sources: vec!["index.rst".to_string()],
            },
            &mut state,
        );
        let paragraph = tree.node(tree.root()).children[0];
        let old = tree.node(paragraph).children[0];
        let first = tree.new_node(kinds::PROBLEMATIC);
        tree.node_mut(first).attrs.ids = vec!["b".to_string(), "z".to_string()];
        let second = tree.new_node("inline");
        tree.replace_self(old, vec![first, second]);
        let merged = &tree.node(first).attrs;
        assert_eq!(merged.ids, ["b", "z", "a"]);
        assert_eq!(merged.classes, ["c"]);
        assert_eq!(merged.names, ["n"]);
        assert_eq!(merged.dupnames, ["d"]);
        assert!(merged.backrefs.is_empty());
        assert_eq!(tree.node(second).attrs, Attrs::default());
        assert_eq!(tree.node(paragraph).children, [first, second]);
        assert_eq!(tree.parent(first), Some(paragraph));
        assert_eq!(tree.parent(second), Some(paragraph));
        // A Text first, or nothing at all: no attribute moves.
        let text = tree.new_node(kinds::TEXT);
        tree.node_mut(text).text = Some("t".to_string());
        tree.replace_self(second, vec![text]);
        assert_eq!(tree.node(paragraph).children, [first, text]);
        tree.replace_self(text, Vec::new());
        assert_eq!(tree.node(paragraph).children, [first]);

        // A detached root is replaced among the forest's roots.
        let mut state = DocumentState::default();
        state
            .detached
            .push(with_ids(kinds::REFERENCE, &["r"], Vec::new()));
        let mut tree = TxTree::from_parse(
            Doctree {
                root: Node::elem(kinds::DOCUMENT, Span::ZERO),
                sources: vec!["index.rst".to_string()],
            },
            &mut state,
        );
        let reference = tree.detached()[0];
        let prb = tree.new_node(kinds::PROBLEMATIC);
        tree.replace_self(reference, vec![prb]);
        assert_eq!(tree.detached(), [prb]);
        assert_eq!(tree.parent(prb), None);
        assert_eq!(tree.node(prb).attrs.ids, ["r"]);
    }

    /// docutils' `Node.findall` and `Node.next_node` on a small document,
    /// from every node and with every flag combination. `findall` yields
    /// the node itself and its descendants in pre-order; `next_node` the
    /// first match of the same walk without the node itself unless asked,
    /// then (siblings) the following siblings and their subtrees, then
    /// (ascend, which implies siblings) the parent's following siblings,
    /// never the parent itself.
    //
    // oracle (docutils 0.22.4, parse layer): the snippet parsed by
    //   `docutils.parsers.rst.Parser` into `utils.new_document`; for each
    //   node `n` of `list(doc.findall())` and each flag combination,
    //   `n.next_node(None, ...)` and `n.next_node(nodes.paragraph, ...)` as
    //   indices into that list, as tabled below; `findall` from the first
    //   section gives indices 1-14, `doc.findall(nodes.paragraph)` 4, 11,
    //   13, 18.
    #[test]
    fn findall_and_next_node_walk_like_docutils() {
        let source = "Title\n=====\n\nPara *one*.\n\n.. _tgt:\n\n.. note:: Inside.\n\n\
                      Last para.\n\nSecond\n======\n\nBody.\n";
        let ParseOutput {
            doctree, mut state, ..
        } = parse_rst_full(
            source,
            &ParseOptions {
                source_path: "<snippet>".to_string(),
                ..ParseOptions::default()
            },
        );
        assert_eq!(
            doctree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <section ids=\"title\" names=\"title\">\n",
                "        <title>\n",
                "            Title\n",
                "        <paragraph>\n",
                "            Para \n",
                "            <emphasis>\n",
                "                one\n",
                "            .\n",
                "        <target ids=\"tgt\" names=\"tgt\">\n",
                "        <note>\n",
                "            <paragraph>\n",
                "                Inside.\n",
                "        <paragraph>\n",
                "            Last para.\n",
                "    <section ids=\"second\" names=\"second\">\n",
                "        <title>\n",
                "            Second\n",
                "        <paragraph>\n",
                "            Body.\n",
            )
        );
        let tree = TxTree::from_parse(doctree, &mut state);
        let nodes = tree.findall(tree.root(), |_| true);
        assert_eq!(nodes.len(), 20);
        let index = |found: Option<TxId>| {
            found.map(|id| nodes.iter().position(|&n| n == id).expect("a node"))
        };
        let paragraph = |node: &TxNode| node.kind == kinds::PARAGRAPH;
        assert_eq!(
            tree.findall(nodes[1], |_| true),
            nodes[1..15].to_vec(),
            "the first section's subtree"
        );
        let paragraphs: Vec<usize> = tree
            .findall(tree.root(), paragraph)
            .into_iter()
            .map(|id| index(Some(id)).expect("found"))
            .collect();
        assert_eq!(paragraphs, [4, 11, 13, 18]);

        // Per start node: `next_node()` and `next_node(nodes.paragraph)`
        // for the flags (include_self, descend, siblings, ascend) counted
        // in binary from (F, F, F, F) to (T, T, T, T); `.` is None.
        #[rustfmt::skip]
        let expected: [(&str, &str); 20] = [
            (". . . . 1 1 1 1 0 0 0 0 0 0 0 0", ". . . . 4 4 4 4 . . . . 4 4 4 4"),
            (". 15 15 15 2 2 2 2 1 1 1 1 1 1 1 1", ". . . . 4 4 4 4 . . . . 4 4 4 4"),
            (". 4 4 4 3 3 3 3 2 2 2 2 2 2 2 2", ". 4 4 4 . 4 4 4 . 4 4 4 . 4 4 4"),
            (". 4 . 4 . 4 . 4 3 3 3 3 3 3 3 3", ". 4 . 4 . 4 . 4 . 4 . 4 . 4 . 4"),
            (". 9 9 9 5 5 5 5 4 4 4 4 4 4 4 4", ". 13 13 13 . 11 11 11 4 4 4 4 4 4 4 4"),
            (". 6 6 6 . 6 6 6 5 5 5 5 5 5 5 5", ". 13 . 13 . 11 . 11 . 13 . 13 . 11 . 11"),
            (". 8 8 8 7 7 7 7 6 6 6 6 6 6 6 6", ". 13 . 13 . 11 . 11 . 13 . 13 . 11 . 11"),
            (". 8 . 8 . 8 . 8 7 7 7 7 7 7 7 7", ". 13 . 13 . 11 . 11 . 13 . 13 . 11 . 11"),
            (". 9 . 9 . 9 . 9 8 8 8 8 8 8 8 8", ". 13 . 13 . 11 . 11 . 13 . 13 . 11 . 11"),
            (". 10 10 10 . 10 10 10 9 9 9 9 9 9 9 9", ". 13 13 13 . 11 11 11 . 13 13 13 . 11 11 11"),
            (". 13 13 13 11 11 11 11 10 10 10 10 10 10 10 10", ". 13 13 13 11 11 11 11 . 13 13 13 11 11 11 11"),
            (". 13 . 13 12 12 12 12 11 11 11 11 11 11 11 11", ". 13 . 13 . 13 . 13 11 11 11 11 11 11 11 11"),
            (". 13 . 13 . 13 . 13 12 12 12 12 12 12 12 12", ". 13 . 13 . 13 . 13 . 13 . 13 . 13 . 13"),
            (". 15 . 15 14 14 14 14 13 13 13 13 13 13 13 13", ". . . . . 18 . 18 13 13 13 13 13 13 13 13"),
            (". 15 . 15 . 15 . 15 14 14 14 14 14 14 14 14", ". . . . . 18 . 18 . . . . . 18 . 18"),
            (". . . . 16 16 16 16 15 15 15 15 15 15 15 15", ". . . . 18 18 18 18 . . . . 18 18 18 18"),
            (". 18 18 18 17 17 17 17 16 16 16 16 16 16 16 16", ". 18 18 18 . 18 18 18 . 18 18 18 . 18 18 18"),
            (". 18 . 18 . 18 . 18 17 17 17 17 17 17 17 17", ". 18 . 18 . 18 . 18 . 18 . 18 . 18 . 18"),
            (". . . . 19 19 19 19 18 18 18 18 18 18 18 18", ". . . . . . . . 18 18 18 18 18 18 18 18"),
            (". . . . . . . . 19 19 19 19 19 19 19 19", ". . . . . . . . . . . . . . . ."),
        ];
        let parse_row = |row: &str| -> Vec<Option<usize>> {
            row.split(' ')
                .map(|token| (token != ".").then(|| token.parse().expect("an index")))
                .collect()
        };
        for (start, (any, paragraphs)) in expected.iter().enumerate() {
            let (any, paragraphs) = (parse_row(any), parse_row(paragraphs));
            for flags in 0..16 {
                let [include_self, descend, siblings, ascend] =
                    [8, 4, 2, 1].map(|bit| flags & bit != 0);
                let walk = |pred: &dyn Fn(&TxNode) -> bool| {
                    index(tree.next_node(
                        nodes[start],
                        pred,
                        include_self,
                        descend,
                        siblings,
                        ascend,
                    ))
                };
                let label = format!(
                    "node {start}, include_self {include_self}, descend {descend}, \
                     siblings {siblings}, ascend {ascend}"
                );
                assert_eq!(walk(&|_| true), any[flags], "{label}");
                assert_eq!(walk(&paragraph), paragraphs[flags], "{label}");
            }
        }
    }

    /// [`Findall`] is docutils' live `findall` generator: a loop over the
    /// document's paragraphs that changes the tree at `Two.` skips the
    /// paragraph that moves into the place it has passed (`Three.` once
    /// `One.` is gone, `X.` once two paragraphs replace `Two.`) and never
    /// reaches one taken out ahead of it (`Four.`).
    //
    // oracle (docutils 0.22.4): `for p in doc.findall(nodes.paragraph)` over
    //   the snippet's parse, doing the same at `Two.`: `doc.remove(doc[0])`
    //   gives One, Two, Four; `p.replace_self([X, Y])` gives One, Two, Y,
    //   Three, Four; `doc.remove(doc[3])` gives One, Two, Three.
    #[test]
    fn findall_walks_the_tree_as_it_changes() {
        let paragraph = |node: &TxNode| node.kind == kinds::PARAGRAPH;
        let at_two = |change: &dyn Fn(&mut TxTree, TxId)| {
            let ParseOutput {
                doctree, mut state, ..
            } = read_parse("One.\n\nTwo.\n\nThree.\n\nFour.\n");
            let mut tree = TxTree::from_parse(doctree, &mut state);
            let mut walk = Findall::new(tree.root());
            let mut seen = Vec::new();
            while let Some(node) = walk.next(&tree, paragraph) {
                let text = tree.astext(node);
                if text == "Two." {
                    change(&mut tree, node);
                }
                seen.push(text);
            }
            seen
        };
        let remove = |at: usize| {
            move |tree: &mut TxTree, _: TxId| {
                let root = tree.root();
                let child = tree.node(root).children[at];
                tree.remove(root, child);
            }
        };
        assert_eq!(at_two(&remove(0)), ["One.", "Two.", "Four."]);
        assert_eq!(at_two(&remove(3)), ["One.", "Two.", "Three."]);
        let replace = |tree: &mut TxTree, two: TxId| {
            let replacements = ["X.", "Y."].map(|text| {
                let paragraph = tree.new_node(kinds::PARAGRAPH);
                let data = tree.new_node(kinds::TEXT);
                tree.node_mut(data).text = Some(text.to_string());
                tree.append(paragraph, data);
                paragraph
            });
            tree.replace_self(two, replacements.to_vec());
        };
        assert_eq!(at_two(&replace), ["One.", "Two.", "Y.", "Three.", "Four."]);
    }
}
