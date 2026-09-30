//! docutils' reference transforms (`docutils/transforms/references.py`),
//! which Sphinx's read phase inherits from the standalone reader, and
//! Sphinx's DefaultSubstitutions, which feeds the first of them. Today:
//! DefaultSubstitutions, Substitutions, PropagateTargets.

use std::collections::{BTreeMap, HashMap};

use super::{dates, for_each_node_mut, node_at, node_at_mut, NodePath, TransformCtx};
use crate::doctree::{kinds, messages, AttrValue, Node, Span, RAWSOURCE};

/// `_DEFAULT_SUBSTITUTIONS` (`sphinx/transforms/__init__.py:47-52`).
const DEFAULT_SUBSTITUTIONS: [&str; 4] = ["version", "release", "today", "translation progress"];

/// `DefaultSubstitutions` (`sphinx/transforms/__init__.py:111-137`,
/// priority 210, ahead of docutils' Substitutions): every
/// `substitution_reference` — in the text and in the definitions alike —
/// whose `refname` is one of `version`, `release`, `today` and
/// `translation progress`, spelled exactly so and not defined by the
/// document itself (`:118-122`: `|Version|` is left to Substitutions, and
/// `|version|` beside a `.. |Version|` definition still takes the default),
/// is replaced by a Text of:
///
/// * `version`/`release`: the config value (`:137`), `''` by default —
///   an empty Text;
/// * `today`: `config.today` when set, else `format_date(today_fmt or
///   '%b %d, %Y', language=...)` of the build date (`:130-135`,
///   [`dates::build_date`] — `$SOURCE_DATE_EPOCH` or now, UTC), formatted
///   in English whatever `language` is (ledgered: Babel's other locales,
///   and Sphinx's translation of the default format, are not ported);
/// * `translation progress`: `_calculate_translation_progress`
///   (`:140-150`) over `document['translation_progress']`, which
///   TranslationProgressTotaliser (025) counts from the `translated` marks
///   only a message catalog's Locale transform leaves; this crate reads no
///   catalogs, so the total is 0 and the text is always
///   `no translated elements!` (untranslated, as for `language='en'`).
///
/// The build date is read once per document (Sphinx reads it once per
/// `|today|`; they differ only if the clock turns between two of them).
pub(super) fn default_substitutions(ctx: &mut TransformCtx) {
    if !contains_substitution_reference(&ctx.tree.root) {
        return;
    }
    let defined = &ctx.lists().substitution_defs;
    let to_handle: Vec<&str> = DEFAULT_SUBSTITUTIONS
        .into_iter()
        .filter(|name| !defined.contains_key(*name))
        .collect();
    let config = ctx.config;
    let mut today: Option<String> = None;
    for_each_node_mut(&mut ctx.tree.root, |node| {
        for child in &mut node.children {
            if child.kind != kinds::SUBSTITUTION_REFERENCE {
                continue;
            }
            let Some(AttrValue::Str(name)) = child.get("refname") else {
                continue;
            };
            if !to_handle.contains(&name.as_str()) {
                continue;
            }
            let text = match name.as_str() {
                "version" => config.version.clone(),
                "release" => config.release.clone(),
                "today" => today
                    .get_or_insert_with(|| {
                        if config.today.is_empty() {
                            let format = config.today_fmt.as_deref().filter(|f| !f.is_empty());
                            dates::format_date(
                                format.unwrap_or("%b %d, %Y"),
                                dates::build_date(config.build_date),
                            )
                        } else {
                            config.today.clone()
                        }
                    })
                    .clone(),
                _ => "no translated elements!".to_string(),
            };
            *child = Node::text_node(text, child.span);
        }
    });
}

/// docutils' `line_length_limit` setting (`docutils/parsers/__init__.py:
/// 76`), which Sphinx leaves at its default.
const LINE_LENGTH_LIMIT: usize = 10_000;

/// `Substitutions` (`docutils/transforms/references.py:642-764`, priority
/// 220): every `substitution_reference` is replaced by a copy of its
/// definition's children, the definition staying where it is.
///
/// The worklist is every reference in document order (`:681`), definitions'
/// own included, and grows as expansions bring nested references in. For
/// each, in turn:
///
/// * the definition is the one named `refname` exactly, else the one whose
///   name matches case-insensitively (`document.substitution_names`,
///   `:685-690`); with none, `Undefined substitution referenced: "%s".`
///   (ERROR, at the reference, `:691-693`);
/// * a definition whose text is by now longer than `line_length_limit`
///   gives `Substitution definition "%s" exceeds the line-length-limit.`
///   (`:694-699`) — an error with no node, located where the parse ended
///   ([`TransformCtx::end_of_parse_message`]);
/// * either error replaces the reference with `problematic(rawsource,
///   refid=<the message's id>)`, message and problematic each taking the
///   document's next id (`:700-707`); the message itself stays out of the
///   tree;
/// * `ltrim`/`rtrim` (`trim`: both) strip the whitespace of the Text on
///   that side of the reference (`:711-720`);
/// * the definition is deep-copied (`:721`), and each reference in the
///   copy is queued, remembering the reference it came from (`ref-origin`,
///   `:729-730`) — unless its definition is already on the chain being
///   expanded (`nested`, `:723-728`): a circular definition. Then, for a
///   reference directly inside a definition, that definition is replaced by
///   `Circular substitution definition detected:` + a literal block of its
///   source, at its line (`:732-740`); for any other, the reference is
///   replaced by a `problematic` for `Circular substitution definition
///   referenced: "%s".`, located at the reference its `ref-origin`s lead
///   back to (`:741-754`);
/// * otherwise the reference is replaced by the copy's children (`:756`).
///   `note_refname` for the copied references (`:757-764`) has no
///   counterpart: the next transform's lists are walked afresh.
///
/// docutils works on nodes, not positions: a reference keeps its `parent`
/// after being replaced, a copy's nested references keep theirs in a copy
/// that a circular definition discards, and a definition keeps being
/// expanded after a "detected" message took its place. The port runs on an
/// [`Arena`] that keeps all of that.
///
/// One input hangs docutils and is stopped here instead: a cycle through a
/// definition whose name differs only in case from a later one's — its
/// references are filed under the other name, so docutils' circularity
/// test never fires; the guard at that test ends it as a circular
/// definition (fix round 1, unit-pinned).
///
/// Two inputs abort the Sphinx build, and are carried on here instead: a
/// copy holding a reference to no definition (`normed[...]` raises
/// `KeyError`, `:726`) queues it like any other, to fail as undefined in
/// its turn; a definition that a "detected" message has already replaced
/// (`parent.index(old)` raises `ValueError`, `nodes.py:1101-1103`) or a
/// discarded copy (no parent) is left where it is, its message printed.
pub(super) fn substitutions(ctx: &mut TransformCtx) {
    if !contains_substitution_reference(&ctx.tree.root) {
        return;
    }
    let lists = ctx.lists();
    let def_paths = lists.substitution_defs.clone();
    let normed = lists.substitution_names.clone();
    let root = std::mem::replace(&mut ctx.tree.root, Node::elem(kinds::DOCUMENT, Span::ZERO));
    let mut arena = Arena::new(root);
    let defs: BTreeMap<String, usize> = def_paths
        .into_iter()
        .filter_map(|(name, path)| Some((name, arena.at_path(&path)?)))
        .collect();
    // `nested`: each name to the definitions whose expansion met it.
    let mut nested: HashMap<String, Vec<String>> = HashMap::new();
    // The same bookkeeping by the definition each nested reference resolves
    // to — the termination guard below.
    let mut resolved_nested: HashMap<String, Vec<String>> = HashMap::new();

    let mut worklist = arena.findall(Arena::ROOT, kinds::SUBSTITUTION_REFERENCE);
    let mut next = 0;
    while let Some(&reference) = worklist.get(next) {
        next += 1;
        let refname = arena.str_attr(reference, "refname").to_string();
        let key = if defs.contains_key(&refname) {
            Some(refname.clone())
        } else {
            normed.get(&refname.to_lowercase()).cloned()
        };
        let failure = match &key {
            None => {
                let (source, line) = arena.location(reference);
                let text = format!("Undefined substitution referenced: \"{refname}\".");
                Some(ctx.message(messages::ERROR, &text, source, Some(line)))
            }
            Some(key) if arena.text_len(defs[key]) > LINE_LENGTH_LIMIT => {
                let text =
                    format!("Substitution definition \"{key}\" exceeds the line-length-limit.");
                Some(ctx.end_of_parse_message(messages::ERROR, &text))
            }
            Some(_) => None,
        };
        if let Some(message) = failure {
            ctx.reporter.report(&message);
            replace_with_problematic(ctx, &mut arena, reference);
            continue;
        }
        let key = key.expect("a definition was found");
        let definition = defs[&key];

        trim_around(&mut arena, reference, definition);
        let copy = arena.deepcopy(definition);
        let mut circular = false;
        for nested_reference in arena.findall(copy, kinds::SUBSTITUTION_REFERENCE) {
            let nested_refname = arena.str_attr(nested_reference, "refname").to_string();
            if let Some(nested_name) = normed.get(&nested_refname.to_lowercase()) {
                // Termination guard — a deliberate divergence: docutils
                // hangs. docutils files a nested reference under its
                // case-folded name's definition (`normed`, the last of the
                // names that fold alike), but expands the definition it
                // names exactly (`:685-686`). When those differ (`|A|` with
                // both `.. |A|` and `.. |a|` defined), a cycle through `A`
                // grows `nested["a"]` with `A` and its neighbours only, the
                // test below never fires, and the worklist grows for ever.
                // For those references alone, docutils' own test also runs
                // on the definition the reference resolves to, and the
                // expansion stops as a circular one. Where every name
                // resolves to itself the two lists agree and nothing
                // changes.
                let resolved = if defs.contains_key(&nested_refname) {
                    &nested_refname
                } else {
                    nested_name
                };
                let revisited = |lists: &HashMap<String, Vec<String>>, name: &String| {
                    lists.get(name).is_some_and(|seen| seen.contains(name))
                };
                if revisited(&nested, nested_name)
                    || (resolved != nested_name && revisited(&resolved_nested, resolved))
                {
                    circular = true;
                    break;
                }
                nested
                    .entry(nested_name.clone())
                    .or_default()
                    .push(key.clone());
                resolved_nested
                    .entry(resolved.clone())
                    .or_default()
                    .push(key.clone());
            }
            arena.slots[nested_reference].origin = Some(reference);
            worklist.push(nested_reference);
        }
        if circular {
            report_circular(ctx, &mut arena, reference, &refname);
            continue;
        }
        let children = arena.slots[copy].kids.clone();
        arena.replace_self(reference, children);
    }
    ctx.tree.root = arena.into_tree();
}

/// Whether any `substitution_reference` is in the tree: most documents
/// have none, and both substitution transforms leave those alone.
fn contains_substitution_reference(root: &Node) -> bool {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind == kinds::SUBSTITUTION_REFERENCE {
            return true;
        }
        stack.extend(&node.children);
    }
    false
}

/// `if 'ltrim' in subdef.attributes or 'trim' in ...` (`references.py:
/// 709-720`): the Text before the reference loses its trailing whitespace,
/// the Text after it its leading (Python's `str.rstrip`/`lstrip`).
fn trim_around(arena: &mut Arena, reference: usize, definition: usize) {
    let Some(parent) = arena.slots[reference].parent else {
        return;
    };
    let Some(index) = arena.slots[parent]
        .kids
        .iter()
        .position(|&k| k == reference)
    else {
        return;
    };
    let has = |key| arena.slots[definition].node.get(key).is_some();
    let (ltrim, rtrim) = (has("ltrim") || has("trim"), has("rtrim") || has("trim"));
    let siblings = arena.slots[parent].kids.clone();
    let before = index.checked_sub(1).map(|i| siblings[i]);
    let after = siblings.get(index + 1).copied();
    for (sibling, trim_end) in [
        (before.filter(|_| ltrim), true),
        (after.filter(|_| rtrim), false),
    ] {
        let Some(sibling) = sibling else { continue };
        let node = &mut arena.slots[sibling].node;
        if node.kind != kinds::TEXT {
            continue;
        }
        if let Some(text) = &mut node.text {
            *text = if trim_end {
                text.trim_end_matches(crate::utils::py_isspace).to_string()
            } else {
                text.trim_start_matches(crate::utils::py_isspace)
                    .to_string()
            };
        }
    }
}

/// `msgid = set_id(msg)`, `problematic(ref.rawsource, ref.rawsource,
/// refid=msgid)`, `set_id(prb)`, `ref.replace_self(prb)`
/// (`references.py:700-707,749-754`). The message's backref to the
/// problematic is not kept: the message is not in the tree.
fn replace_with_problematic(ctx: &mut TransformCtx, arena: &mut Arena, reference: usize) {
    let message_id = ctx.ids.allocate_auto_id();
    let span = arena.slots[reference].node.span;
    let rawsource = arena.str_attr(reference, RAWSOURCE).to_string();
    let mut problematic = Node::elem(kinds::PROBLEMATIC, span);
    problematic.set("refid", AttrValue::Str(message_id));
    problematic.children.push(Node::text_node(rawsource, span));
    problematic.attrs.ids.push(ctx.ids.allocate_auto_id());
    let problematic = arena.adopt(problematic, None);
    arena.replace_self(reference, vec![problematic]);
}

/// The `CircularSubstitutionDefinitionError` branch (`references.py:
/// 731-755`).
fn report_circular(ctx: &mut TransformCtx, arena: &mut Arena, reference: usize, refname: &str) {
    match arena.slots[reference].parent {
        Some(definition) if arena.slots[definition].node.kind == "substitution_definition" => {
            let span = arena.slots[definition].node.span;
            let rawsource = arena.str_attr(definition, RAWSOURCE).to_string();
            let message = messages::with_literal(
                ctx.message(
                    messages::ERROR,
                    "Circular substitution definition detected:",
                    span.source,
                    Some(span.line),
                ),
                &rawsource,
            );
            ctx.reporter.report(&message);
            let message = arena.adopt(message, None);
            arena.replace_self(definition, vec![message]);
        }
        _ => {
            let mut origin = reference;
            while let Some(earlier) = arena.slots[origin].origin {
                origin = earlier;
            }
            let (source, line) = arena.location(origin);
            let text = format!("Circular substitution definition referenced: \"{refname}\".");
            ctx.reporter
                .report(&ctx.message(messages::ERROR, &text, source, Some(line)));
            replace_with_problematic(ctx, arena, reference);
        }
    }
}

/// A doctree the way docutils holds one: every node reachable by index and
/// knowing its parent, the way docutils' `node.parent` does — which
/// `Element.replace` never clears (`nodes.py:1101-1109`), so a node taken
/// out of its parent's children still points at it. Substitutions
/// (`references.py:674-764`) depends on that: it expands references inside
/// copies and definitions that are no longer in the tree, and locates
/// references by ancestors they have left. Every walk is by explicit stack.
struct Arena {
    slots: Vec<Slot>,
}

struct Slot {
    /// The node itself, its children held in `kids` instead.
    node: Node,
    kids: Vec<usize>,
    parent: Option<usize>,
    /// `ref['ref-origin']` (`references.py:729`): the reference whose
    /// expansion brought this nested one in. An attribute in docutils, so a
    /// deep copy carries it.
    origin: Option<usize>,
}

impl Arena {
    /// The document the arena was made from.
    const ROOT: usize = 0;

    fn new(root: Node) -> Arena {
        let mut arena = Arena { slots: Vec::new() };
        arena.adopt(root, None);
        arena
    }

    /// Move `node` and its subtree in, in document order, under `parent`
    /// (its last child); returns the node's index.
    fn adopt(&mut self, node: Node, parent: Option<usize>) -> usize {
        let first = self.slots.len();
        let mut stack = vec![(node, parent)];
        while let Some((mut node, parent)) = stack.pop() {
            let id = self.slots.len();
            let children = std::mem::take(&mut node.children);
            self.slots.push(Slot {
                node,
                kids: Vec::with_capacity(children.len()),
                parent,
                origin: None,
            });
            if let Some(parent) = parent {
                self.slots[parent].kids.push(id);
            }
            stack.extend(children.into_iter().rev().map(|child| (child, Some(id))));
        }
        first
    }

    /// `node.deepcopy()` (`nodes.py:1197-1201`): a parentless copy of the
    /// subtree, attributes (and `ref-origin`s) included; returns its index.
    fn deepcopy(&mut self, id: usize) -> usize {
        let first = self.slots.len();
        let mut stack = vec![(id, None)];
        while let Some((original, parent)) = stack.pop() {
            let copy = self.slots.len();
            let slot = &self.slots[original];
            let kids = slot.kids.clone();
            self.slots.push(Slot {
                node: slot.node.clone(),
                kids: Vec::with_capacity(kids.len()),
                parent,
                origin: slot.origin,
            });
            if let Some(parent) = parent {
                self.slots[parent].kids.push(copy);
            }
            stack.extend(kids.into_iter().rev().map(|kid| (kid, Some(copy))));
        }
        first
    }

    /// `findall(kind)` from `id`: every node of that kind in its subtree, in
    /// document order.
    fn findall(&self, id: usize, kind: &str) -> Vec<usize> {
        let mut found = Vec::new();
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            if self.slots[id].node.kind == kind {
                found.push(id);
            }
            stack.extend(self.slots[id].kids.iter().rev());
        }
        found
    }

    /// The node at `path` below the root.
    fn at_path(&self, path: &[usize]) -> Option<usize> {
        path.iter().try_fold(Self::ROOT, |id, &index| {
            self.slots[id].kids.get(index).copied()
        })
    }

    /// `len(node.astext())`, in characters, as Python counts them.
    fn text_len(&self, id: usize) -> usize {
        let mut len = 0;
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            if let Some(text) = &self.slots[id].node.text {
                len += text.chars().count();
            }
            stack.extend(&self.slots[id].kids);
        }
        len
    }

    fn str_attr(&self, id: usize, key: &'static str) -> &str {
        match self.slots[id].node.get(key) {
            Some(AttrValue::Str(value)) => value,
            _ => "",
        }
    }

    /// Where docutils locates a substitution reference: `get_source_line`
    /// (`docutils/utils/__init__.py:645-654`) walks up to the first
    /// ancestor carrying a line, the inliner stamping none on the reference
    /// or on the `reference` a `|name|_` wraps it in — a paragraph, a
    /// definition (its marker line), a table cell's paragraph; a section
    /// title is unstamped too (`new_subsection`, `states.py:499-503`), so
    /// there it is the section's line. Elsewhere the enclosing element's
    /// span line stands in for docutils' `line` (ledgered where they differ:
    /// an attribution, a field name, a glossary term).
    fn location(&self, reference: usize) -> (u16, u32) {
        let mut at = reference;
        while let Some(parent) = self.slots[at].parent {
            at = parent;
            if self.slots[at].node.kind != kinds::REFERENCE {
                break;
            }
        }
        if self.slots[at].node.kind == kinds::TITLE {
            if let Some(section) = self.slots[at]
                .parent
                .filter(|&parent| self.slots[parent].node.kind == kinds::SECTION)
            {
                at = section;
            }
        }
        let span = self.slots[at].node.span;
        (span.source, span.line)
    }

    /// `old.replace_self(new)` (`nodes.py:1110-1132`): the first new node,
    /// if an element, takes on `old`'s ids, classes, names and dupnames
    /// (`update_basic_atts`, `:850-869`, skipping values it has), and the new
    /// nodes take `old`'s place in its parent (`Element.replace`,
    /// `:1101-1109`), which becomes theirs. Does nothing when `old` has no
    /// parent or its parent no longer holds it, where docutils raises.
    fn replace_self(&mut self, old: usize, new: Vec<usize>) {
        let Some(parent) = self.slots[old].parent else {
            return;
        };
        let Some(index) = self.slots[parent].kids.iter().position(|&kid| kid == old) else {
            return;
        };
        if let Some(&first) = new.first() {
            if self.slots[first].node.kind != kinds::TEXT {
                let old_attrs = self.slots[old].node.attrs.clone();
                let attrs = &mut self.slots[first].node.attrs;
                for (list, values) in [
                    (&mut attrs.ids, &old_attrs.ids),
                    (&mut attrs.classes, &old_attrs.classes),
                    (&mut attrs.names, &old_attrs.names),
                    (&mut attrs.dupnames, &old_attrs.dupnames),
                ] {
                    for value in values {
                        if !list.contains(value) {
                            list.push(value.clone());
                        }
                    }
                }
            }
        }
        for &node in &new {
            self.slots[node].parent = Some(parent);
        }
        self.slots[parent].kids.splice(index..=index, new);
    }

    /// The tree under the root, rebuilt from the leaves up.
    fn into_tree(mut self) -> Node {
        let mut order = Vec::new();
        let mut stack = vec![Self::ROOT];
        while let Some(id) = stack.pop() {
            order.push(id);
            stack.extend(self.slots[id].kids.iter().rev());
        }
        let mut built: Vec<Option<Node>> = Vec::new();
        built.resize_with(self.slots.len(), || None);
        for &id in order.iter().rev() {
            let placeholder = Node::elem(kinds::TEXT, Span::ZERO);
            let mut node = std::mem::replace(&mut self.slots[id].node, placeholder);
            node.children = self.slots[id]
                .kids
                .iter()
                .filter_map(|&kid| built[kid].take())
                .collect();
            built[id] = Some(node);
        }
        built[Self::ROOT].take().expect("the root is built last")
    }
}

/// `PropagateTargets` (`docutils/transforms/references.py:17-95`, priority
/// 260): every block-level target without a reference of its own — a
/// `.. _label:`, an anonymous `.. __:`, the target of an `index` or
/// `py:module` directive — hands its `ids` and `names` on to the node after
/// it, *appended* to that node's own (`:60-61`), and keeps only
/// `refid = <its first id>` (`:91-95`). Targets are taken in document order
/// (`findall(nodes.target)`, `:43`), so a run of targets collapses onto the
/// last one first and then onto the node behind the run: `.. _a:` `.. _b:`
/// para gives `<target refid="a"><target refid="b"><paragraph ids="b a">`.
///
/// Skipped (`:45-48`): a target inside a `TextElement` — an inline target,
/// the one an `:index:` or `:envvar:` role puts beside its text (in a
/// paragraph, a line, a title…), and any target in a `versionmodified`
/// body, whose content sits directly in that TextElement (probed) — and one
/// that already carries `refid`, `refuri` or `refname`.
///
/// The receiving node is `target.next_node(ascend=True)` (`:50`) — the next
/// sibling, else the nearest ancestor's next sibling — stepping over
/// `system_message`s whole (`next_node(ascend=True, descend=False)`,
/// `:52-53`): they are still in the tree at 260, FilterSystemMessages only
/// runs at 999. Nothing is propagated when there is no such node, or when it
/// is `Invisible` (comment, substitution definition, pending, index) or
/// `Targetable` (footnote, citation) other than a target (`:56-59`).
///
/// A target that is a `figure`'s child followed by the figure's `caption`
/// is removed instead of keeping a `refid` (`:85-88`). The removals are
/// made after the walk: the walk only ever looks forward from a target, so
/// a removed target is never in the way of a later one, and the caption
/// docutils' iterator skips after the removal holds no target the walk
/// would propagate (its targets have a TextElement parent).
///
/// `document.ids` (`:67-69`) and `document.refids` (`note_refid`, `:95`)
/// are the walk-built [`super::DocumentLists`], which see the moved ids
/// and the new `refid`s. Not ported: the `expect_referenced_by_*`
/// bookkeeping (`:62-80`), which only marks targets `referenced` for
/// DanglingReferences' "Hyperlink target is not referenced" INFO — a
/// message Sphinx's SphinxDanglingReferences never lets through
/// (`sphinx/transforms/references.py:21-30`).
pub(super) fn propagate_targets(ctx: &mut TransformCtx) {
    let root = &mut ctx.tree.root;
    let mut removed: Vec<NodePath> = Vec::new();
    for path in collect_targets(root) {
        let Some((target, parent)) = node_at(root, &path).zip(parent_of(root, &path)) else {
            continue;
        };
        if is_text_element(parent.kind)
            || ["refid", "refuri", "refname"]
                .into_iter()
                .any(|key| target.get(key).is_some())
        {
            continue;
        }
        // `assert len(target) == 0` (`:49`) — upstream aborts on a
        // block-level target with children; none reaches here, and one that
        // did is left as it is. `target['ids'][0]` (`:91`) raises on a target
        // without ids, which no directive produces either.
        if !target.children.is_empty() || target.attrs.ids.is_empty() {
            continue;
        }
        let mut next = following(root, &path);
        while let Some(message) = next
            .as_ref()
            .filter(|next| node_at(root, next).map(|n| n.kind) == Some(kinds::SYSTEM_MESSAGE))
        {
            next = following(root, message);
        }
        let Some(next) = next else { continue };
        let Some(receiver) = node_at(root, &next) else {
            continue;
        };
        // A text node cannot carry ids; upstream would fail on it.
        if receiver.kind == kinds::TEXT
            || ((is_invisible(receiver.kind) || is_targetable(receiver.kind))
                && receiver.kind != kinds::TARGET)
        {
            continue;
        }
        let to_caption = parent.kind == "figure" && receiver.kind == "caption";

        let target = node_at_mut(root, &path).expect("the target was just read");
        let ids = std::mem::take(&mut target.attrs.ids);
        let names = std::mem::take(&mut target.attrs.names);
        if to_caption {
            removed.push(path);
        } else {
            target.set("refid", AttrValue::Str(ids[0].clone()));
        }
        let receiver = node_at_mut(root, &next).expect("the receiver was just read");
        receiver.attrs.ids.extend(ids);
        receiver.attrs.names.extend(names);
    }

    // Last first, so each path still names its target.
    for path in removed.iter().rev() {
        if let Some((index, parent)) = path.split_last() {
            if let Some(parent) = node_at_mut(root, parent) {
                parent.children.remove(*index);
            }
        }
    }
}

/// Every `target` below `root`, in document order (pre-order, by an
/// explicit stack rather than recursion).
fn collect_targets(root: &Node) -> Vec<NodePath> {
    let mut targets = Vec::new();
    let mut stack: Vec<(&Node, NodePath)> = vec![(root, Vec::new())];
    while let Some((node, path)) = stack.pop() {
        for (index, child) in node.children.iter().enumerate().rev() {
            let mut child_path = path.clone();
            child_path.push(index);
            stack.push((child, child_path));
        }
        if node.kind == kinds::TARGET {
            targets.push(path);
        }
    }
    targets
}

fn parent_of<'n>(root: &'n Node, path: &[usize]) -> Option<&'n Node> {
    let (_, parent) = path.split_last()?;
    node_at(root, parent)
}

/// `next_node(ascend=True, descend=False)` (`docutils/nodes.py:264-367`):
/// the node's next sibling, else its parent's, and so on up — the node
/// after `path` in document order once its own subtree is skipped. For a
/// target, which has no children, this is `next_node(ascend=True)` too.
fn following(root: &Node, path: &[usize]) -> Option<NodePath> {
    let mut path = path.to_vec();
    while let Some(index) = path.pop() {
        if index + 1 < node_at(root, &path)?.children.len() {
            path.push(index + 1);
            return Some(path);
        }
    }
    None
}

/// `isinstance(node, nodes.TextElement)` by tagname: every docutils and
/// Sphinx (`sphinx/addnodes.py`) element class deriving from it, as probed
/// from the installed docutils 0.22.4 / Sphinx 9.1.0 (research
/// `2026-09-30-m2-wave5-transforms.md` Appendix A).
fn is_text_element(kind: &str) -> bool {
    matches!(
        kind,
        "abbreviation"
            | "acronym"
            | "address"
            | "attribution"
            | "author"
            | "caption"
            | "centered"
            | "citation_reference"
            | "classifier"
            | "comment"
            | "compact_paragraph"
            | "contact"
            | "copyright"
            | "date"
            | "desc_addname"
            | "desc_annotation"
            | "desc_inline"
            | "desc_name"
            | "desc_optional"
            | "desc_parameter"
            | "desc_parameterlist"
            | "desc_returns"
            | "desc_sig_keyword"
            | "desc_sig_keyword_type"
            | "desc_sig_literal_char"
            | "desc_sig_literal_number"
            | "desc_sig_literal_string"
            | "desc_sig_name"
            | "desc_sig_operator"
            | "desc_sig_punctuation"
            | "desc_sig_space"
            | "desc_signature"
            | "desc_signature_line"
            | "desc_type"
            | "desc_type_parameter"
            | "desc_type_parameter_list"
            | "doctest_block"
            | "download_reference"
            | "emphasis"
            | "field_name"
            | "footnote_reference"
            | "generated"
            | "index"
            | "inline"
            | "label"
            | "line"
            | "literal"
            | "literal_block"
            | "literal_emphasis"
            | "literal_strong"
            | "manpage"
            | "math"
            | "math_block"
            | "number_reference"
            | "option_argument"
            | "option_string"
            | "organization"
            | "paragraph"
            | "pending_xref_condition"
            | "problematic"
            | "production"
            | "raw"
            | "reference"
            | "revision"
            | "rubric"
            | "status"
            | "strong"
            | "subscript"
            | "substitution_definition"
            | "substitution_reference"
            | "subtitle"
            | "superscript"
            | "target"
            | "term"
            | "title"
            | "title_reference"
            | "version"
            | "versionmodified"
    )
}

/// `isinstance(node, nodes.Invisible)`: docutils' comment,
/// substitution_definition, pending and target, and Sphinx's `index`
/// (probed as for [`is_text_element`]).
fn is_invisible(kind: &str) -> bool {
    matches!(
        kind,
        kinds::COMMENT | "substitution_definition" | "pending" | kinds::TARGET | "index"
    )
}

/// `isinstance(node, nodes.Targetable)`: footnote, citation and target.
fn is_targetable(kind: &str) -> bool {
    matches!(kind, kinds::FOOTNOTE | kinds::CITATION | kinds::TARGET)
}

#[cfg(test)]
mod tests {
    use crate::doctree::ids::IdRegistry;
    use crate::doctree::{kinds, Doctree, Node, Span};
    use crate::rst::ParseOptions;
    use crate::transforms::{apply_read_transforms, parse_and_transform, TransformConfig};

    /// A Sphinx read of `source` with `keep_warnings` on (so the tree keeps
    /// the messages a transform places, as the oracle's does): the
    /// transformed tree and each printed record's `(line, text)`.
    fn read(source: &str) -> (Doctree, Vec<(Option<u32>, String)>) {
        let opts = ParseOptions {
            source_path: "<snippet>".to_string(),
            sphinx: true,
            ..Default::default()
        };
        let config = TransformConfig {
            keep_warnings: true,
            ..TransformConfig::default()
        };
        let (tree, records) = parse_and_transform(source, &opts, &config);
        let records = records.into_iter().map(|d| (d.line, d.text)).collect();
        (tree, records)
    }

    fn contains_kind(node: &Node, kind: &str) -> bool {
        node.kind == kind || node.children.iter().any(|child| contains_kind(child, kind))
    }

    fn undefined(name: &str) -> String {
        format!("Undefined substitution referenced: \"{name}\".")
    }

    /// Three definitions whose last grows to 10 * 10 * 100 characters plus
    /// separators — past `line_length_limit` (10000) — before `|c|`, below
    /// them, expands it (the inputs of the probes behind research §9.3).
    fn line_length_input(tail: &str) -> String {
        let a = "x".repeat(100);
        let b = ["|a|"; 10].join(" ");
        let c = ["|b|"; 10].join(" ");
        format!(".. |a| replace:: {a}\n.. |b| replace:: {b}\n.. |c| replace:: {c}\n\nSee |c|{tail}")
    }

    /// The line-length error has no node to locate it by
    /// (`references.py:697-699`): docutils' reporter asks its finished state
    /// machine, whose cursor has run one past the last input line —
    /// trailing blank lines counted, a missing final newline not (probed:
    /// lines 6, 9 and 6).
    #[test]
    fn the_line_length_error_is_located_one_past_the_last_line() {
        let expected = |line| {
            vec![(
                Some(line),
                "Substitution definition \"c\" exceeds the line-length-limit.".to_string(),
            )]
        };
        for (tail, line) in [(" here.\n", 6), (" here.\n\n\n\n", 9), (" here.", 6)] {
            let (tree, records) = read(&line_length_input(tail));
            assert_eq!(records, expected(line), "{tail:?}");
            let paragraph = tree.root.children.last().unwrap();
            assert_eq!(paragraph.children[1].kind, kinds::PROBLEMATIC);
        }
    }

    /// `get_source_line(ref)` (`docutils/utils/__init__.py:645-654`): the
    /// inliner stamps no line on a reference, so docutils reports the
    /// nearest stamped ancestor's — a paragraph's first line, a definition's
    /// marker line (not its content's), a table cell paragraph's line, and
    /// for a section title (`new_subsection` leaves it unstamped,
    /// `states.py:499-503`) the section's. Probed.
    #[test]
    fn a_reference_is_located_where_docutils_finds_it() {
        for (source, line) in [
            ("First line\nsecond |u| line.\n", 1),
            (".. |a| replace::\n   x |u|\n", 1),
            ("+-------+\n| |u|   |\n+-------+\n", 2),
            ("|u|\n===\n", 2),
        ] {
            let (_, records) = read(source);
            assert_eq!(records, [(Some(line), undefined("u"))], "{source:?}");
        }
    }

    /// The `problematic` holds the reference's rawsource, backslashes and
    /// all, while the message names its (unescaped) refname — probed:
    /// `|a\ b|` and `"ab"`.
    #[test]
    fn a_problematic_holds_the_reference_as_written() {
        let (tree, records) = read("See |a\\ b| here.\n");
        assert_eq!(records, [(Some(1), undefined("ab"))]);
        assert_eq!(
            tree.root.children[0].pformat(),
            "<paragraph>\n    See \n    <problematic ids=\"id2\" refid=\"id1\">\n        \
             |a\\ b|\n     here.\n"
        );
    }

    /// A definition holding an undefined reference, expanded before its own
    /// reference is reached: docutils looks the nested name up without a
    /// default (`normed[...]`, `references.py:726`) and the `KeyError` aborts
    /// the Sphinx build (probed). Here the nested copy is queued like any
    /// other and fails as undefined when its turn comes — after the
    /// definition's own reference.
    #[test]
    fn a_nested_undefined_reference_is_an_error_where_sphinx_crashes() {
        let (tree, records) = read("|a|\n\n.. |a| replace:: x |nope|\n");
        assert_eq!(
            records,
            [(Some(3), undefined("nope")), (Some(1), undefined("nope"))]
        );
        assert!(!contains_kind(&tree.root, kinds::SUBSTITUTION_REFERENCE));
    }

    /// Two definition names differing only in case (review finding, fix
    /// round 1): `|A|` resolves to `A` exactly (`references.py:685-686`),
    /// but docutils files each nested reference under its case-folded
    /// name's definition (`normed[...]`, `:726-728`) — here `a`, whose list
    /// only ever gains `b` and `A` — so its circularity test never fires and
    /// the expansion never ends (docutils 0.22.4 hangs: probed by the
    /// review, `timeout 20` exit 124; no oracle case can exist). The guard
    /// stops it with the ordinary circular errors: the paragraph's
    /// reference "referenced", then each definition "detected".
    #[test]
    fn a_case_folded_cycle_ends_where_docutils_never_does() {
        const SOURCE: &str =
            ".. |A| replace:: |b|\n.. |b| replace:: |A|\n.. |a| replace:: z\n\nSee |A|.\n";
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(read(SOURCE));
        });
        let (tree, records) = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the read pass did not terminate");
        let detected = |line, source: &str| {
            (
                Some(line),
                format!("Circular substitution definition detected:\n\n{source}"),
            )
        };
        assert_eq!(
            records,
            [
                (
                    Some(5),
                    "Circular substitution definition referenced: \"A\".".to_string()
                ),
                detected(1, ".. |A| replace:: |b|"),
                detected(2, ".. |b| replace:: |A|"),
            ]
        );
        assert_eq!(
            tree.root.pformat(),
            "<document source=\"<snippet>\">\n\
             \x20   <system_message level=\"3\" line=\"1\" names=\"A\" source=\"<snippet>\" type=\"ERROR\">\n\
             \x20       <paragraph>\n\
             \x20           Circular substitution definition detected:\n\
             \x20       <literal_block xml:space=\"preserve\">\n\
             \x20           .. |A| replace:: |b|\n\
             \x20   <system_message level=\"3\" line=\"2\" names=\"b\" source=\"<snippet>\" type=\"ERROR\">\n\
             \x20       <paragraph>\n\
             \x20           Circular substitution definition detected:\n\
             \x20       <literal_block xml:space=\"preserve\">\n\
             \x20           .. |b| replace:: |A|\n\
             \x20   <substitution_definition names=\"a\">\n\
             \x20       z\n\
             \x20   <paragraph>\n\
             \x20       See \n\
             \x20       <problematic ids=\"id2\" refid=\"id1\">\n\
             \x20           |A|\n\
             \x20       .\n"
        );
    }

    /// A definition referencing itself twice: its first reference replaces
    /// it with the "detected" message, and its second finds it again,
    /// already out of the tree — where docutils' `parent.index(old)` raises
    /// (`nodes.py:1101-1103`) and the Sphinx build aborts after printing both
    /// messages (probed). Here the second replacement is skipped, every
    /// reference is still replaced, and the pass goes on.
    #[test]
    fn a_second_circular_error_in_a_replaced_definition_is_reported_where_sphinx_crashes() {
        let (tree, records) = read(".. |a| replace:: |a| |a|\n\nSee |a|.\n");
        let detected = (
            Some(1),
            "Circular substitution definition detected:\n\n.. |a| replace:: |a| |a|".to_string(),
        );
        assert_eq!(records[..2], [detected.clone(), detected]);
        assert!(!contains_kind(&tree.root, kinds::SUBSTITUTION_REFERENCE));
        assert_eq!(tree.root.children[0].kind, kinds::SYSTEM_MESSAGE);
        assert!(contains_kind(&tree.root.children[1], kinds::PROBLEMATIC));
    }

    fn elem(kind: &'static str, children: Vec<Node>) -> Node {
        let mut node = Node::elem(kind, Span::ZERO);
        node.children = children;
        node
    }

    /// The one structural change PropagateTargets makes (`references.py:
    /// 85-88`): a target that is a `figure`'s child and whose next node is
    /// the figure's `caption` hands its ids and names over and is removed,
    /// instead of staying behind with a `refid`. No directive puts a target
    /// there (a figure holds its image, then caption and legend), so the
    /// tree is built by hand.
    #[test]
    fn a_target_before_a_figure_caption_is_removed() {
        let mut target = Node::elem(kinds::TARGET, Span::ZERO);
        target.attrs.ids = vec!["t".to_string()];
        target.attrs.names = vec!["t".to_string()];
        let caption = elem("caption", vec![Node::text_node("Cap", Span::ZERO)]);
        let figure = elem(
            "figure",
            vec![elem(kinds::IMAGE, Vec::new()), target, caption],
        );
        let mut tree = Doctree {
            root: elem(kinds::DOCUMENT, vec![figure]),
            sources: vec!["<snippet>".to_string()],
        };
        apply_read_transforms(
            &mut tree,
            IdRegistry::new(),
            0,
            None,
            "index",
            &TransformConfig::default(),
            &mut Vec::new(),
        );
        let figure = &tree.root.children[0];
        let kinds: Vec<&str> = figure.children.iter().map(|child| child.kind).collect();
        assert_eq!(kinds, [kinds::IMAGE, "caption"], "the target is gone");
        assert_eq!(figure.children[1].attrs.ids, ["t"]);
        assert_eq!(figure.children[1].attrs.names, ["t"]);
    }
}
