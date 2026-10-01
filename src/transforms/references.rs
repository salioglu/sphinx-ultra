//! docutils' reference transforms (`docutils/transforms/references.py`),
//! which Sphinx's read phase inherits from the standalone reader, and
//! Sphinx's DefaultSubstitutions, which feeds the first of them, and its
//! SphinxDanglingReferences, which ends them: DefaultSubstitutions,
//! Substitutions, PropagateTargets, AnonymousHyperlinks,
//! IndirectHyperlinks, ExternalTargets, InternalTargets,
//! SphinxDanglingReferences.
//!
//! The hyperlink transforms mark what they resolve `resolved`, a Python
//! attribute outside the tree, which later ones test before touching a
//! reference again. Nothing here keeps it from one transform to the next:
//! every node they mark had no `refname` to begin with (an anonymous
//! reference) or loses its `refname` (or `refid`) in the same step
//! (`references.py:146-159,315-335,371-373,411-413,945-948`, and
//! Footnotes', `:524-529,630-634`), and the walk-built lists the next
//! transform reads ([`super::DocumentLists`]) hold only nodes that still
//! carry one. The three exceptions are a target IndirectHyperlinks failed,
//! which keeps its `refname` (`:296-298`) — the transforms after it skip
//! every `target` still carrying one —, the labelled footnote reference
//! Footnotes numbers from an unlabelled footnote (`:561-568`), which keeps
//! its `refname` beside the `refid` it gains — skipped by that pair
//! ([`footnote_resolved_by_number`]) — and InternalTargets' name without an
//! id (`:409-413`), which is unreachable: every name left in a target's
//! `names` maps to an id (`nodes.py:1929-1990` dupnames the others).

use std::collections::{BTreeMap, HashMap, HashSet};

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
/// Where docutils never finishes, or aborts the Sphinx build, the port
/// ends instead — and only there; every document docutils finishes gets
/// docutils' records and tree:
///
/// * **Never finishes.** docutils files a nested reference under its
///   case-folded name's definition (`normed`, the last of the names that
///   fold alike) but expands the definition it names exactly (`:685-686`).
///   With names differing only in case (`.. |A|` and `.. |a|`), a cycle
///   through `A` may never trip the circularity test, and when no
///   definition on it grows, the line-length limit never ends it either:
///   docutils loops for ever. The backstop ([`ExpansionState`]) ends the
///   expansion once its state repeats — which is proof that docutils would
///   loop for ever, so no document docutils finishes can reach it — each
///   pending reference then taking the circular branch, up to the first
///   "detected" message that cannot take its definition's place (below).
/// * **Aborts, stopped.** A "detected" message that cannot take its
///   definition's place — the definition already replaced by an earlier
///   one (`parent.index(old)` raises `ValueError`, `nodes.py:1101-1103`)
///   or a discarded copy (no parent: `AttributeError`) — is where the
///   Sphinx build aborts, having printed it. The expansion stops there too:
///   the same records printed, the references not yet reached left in the
///   tree unexpanded (expanding on can loop for ever).
/// * **Aborts, reported and carried on.** A copy holding a reference to no
///   definition (`normed[...]` raises `KeyError`, `:726` — a typo inside a
///   definition used before it) is queued like any other reference, with
///   no circularity bookkeeping, and fails as docutils fails an undefined
///   reference in its turn: `Undefined substitution referenced: "%s".` at
///   the reference's place, a `problematic` in its stead; everything else
///   is expanded as usual.
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

    let mut worklist = arena.findall(Arena::ROOT, kinds::SUBSTITUTION_REFERENCE);
    // The backstop: the state at the start of every round of expansions
    // after the first (a round: the references the one before queued).
    let mut round_end = worklist.len();
    let mut states: HashSet<blake3::Hash> = HashSet::new();
    let mut next = 0;
    while let Some(&reference) = worklist.get(next) {
        if next == round_end {
            round_end = worklist.len();
            let pending = &worklist[next..];
            if !states.insert(ExpansionState::digest(&arena, &defs, &nested, pending)) {
                for &reference in pending {
                    let refname = arena.str_attr(reference, "refname").to_string();
                    if !report_circular(ctx, &mut arena, reference, &refname) {
                        break; // as below, where docutils raises
                    }
                }
                break;
            }
        }
        #[cfg(test)]
        assert!(
            worklist.len() < 100_000,
            "runaway substitution expansion (a unit test's input must end)"
        );
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
            let nested_refname = arena.str_attr(nested_reference, "refname").to_lowercase();
            // No definition: docutils' `KeyError` (see above) — queued all
            // the same, to fail as undefined in its turn.
            if let Some(nested_name) = normed.get(&nested_refname) {
                let seen = nested.entry(nested_name.clone()).or_default();
                if seen.contains(nested_name) {
                    circular = true;
                    break;
                }
                seen.push(key.clone());
            }
            arena.slots[nested_reference].origin = Some(reference);
            worklist.push(nested_reference);
        }
        if circular {
            if !report_circular(ctx, &mut arena, reference, &refname) {
                // Where the Sphinx build aborts (see above).
                break;
            }
            continue;
        }
        let children = arena.slots[copy].kids.clone();
        // A reference is always in its parent: it is only replaced here.
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
    let mut problematic = problematic_for(&arena.slots[reference].node, message_id);
    problematic.attrs.ids.push(ctx.ids.allocate_auto_id());
    let problematic = arena.adopt(problematic, None);
    arena.replace_self(reference, vec![problematic]);
}

/// `nodes.problematic(node.rawsource, node.rawsource, refid=msgid)`
/// (`references.py:148-149,292-293,981`, and Substitutions' `:702-703`):
/// the node as written ([`RAWSOURCE`]), pointing at its message. Its own id
/// is the caller's to give.
pub(super) fn problematic_for(node: &Node, message_id: String) -> Node {
    let rawsource = match node.get(RAWSOURCE) {
        Some(AttrValue::Str(rawsource)) => rawsource.clone(),
        _ => String::new(),
    };
    let mut problematic = Node::elem(kinds::PROBLEMATIC, node.span);
    problematic.set("refid", AttrValue::Str(message_id));
    problematic
        .children
        .push(Node::text_node(rawsource, node.span));
    problematic
}

/// `update_basic_atts` (`nodes.py:850-869`) as `replace_self` calls it
/// (`:1120-1132`): the element replacing `old` takes on `old`'s ids,
/// classes, names and dupnames, after its own, skipping values it has.
fn update_basic_atts(new: &mut Node, old: &Node) {
    let attrs = &mut new.attrs;
    for (list, values) in [
        (&mut attrs.ids, &old.attrs.ids),
        (&mut attrs.classes, &old.attrs.classes),
        (&mut attrs.names, &old.attrs.names),
        (&mut attrs.dupnames, &old.attrs.dupnames),
    ] {
        for value in values {
            if !list.contains(value) {
                list.push(value.clone());
            }
        }
    }
}

/// The `CircularSubstitutionDefinitionError` branch (`references.py:
/// 731-755`). `false` when the "detected" message cannot take its
/// definition's place, where docutils raises.
fn report_circular(
    ctx: &mut TransformCtx,
    arena: &mut Arena,
    reference: usize,
    refname: &str,
) -> bool {
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
            arena.replace_self(definition, vec![message])
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
            true
        }
    }
}

/// The backstop that ends an expansion docutils would never finish: a
/// digest of everything the rest of the expansion depends on, taken at the
/// start of each round (the references the round before queued, in
/// worklist order). docutils' algorithm is deterministic in exactly this:
///
/// * each definition by name — its shape, text and references (what a copy
///   holds, the length the line-length limit reads, the names met), its
///   trim flags, and whether it is still in the tree (where a "detected"
///   message can still take its place);
/// * `nested`, as sets (the circularity test asks membership only);
/// * each pending reference, in order: its name, and where it sits — in
///   which definition and at which place in it (what its replacement
///   changes; whether its parent is the definition), in some other
///   definition node (an older duplicate, a discarded copy: which one,
///   still in the tree or not, and the place), or in the text (where
///   nothing it changes is ever read again).
///
/// Ids, message locations and the text's content are output only. So when
/// a round starts in a state an earlier round started in, every round after
/// repeats the rounds since, for ever: docutils would never finish, and no
/// document it finishes can come here. (A digest collision would need two
/// different states to hash alike under BLAKE3.)
struct ExpansionState(blake3::Hasher);

impl ExpansionState {
    fn digest(
        arena: &Arena,
        defs: &BTreeMap<String, usize>,
        nested: &HashMap<String, Vec<String>>,
        pending: &[usize],
    ) -> blake3::Hash {
        let mut state = ExpansionState(blake3::Hasher::new());
        for (name, &definition) in defs {
            state.text(name);
            state.number(usize::from(arena.is_attached(definition)));
            for flag in ["ltrim", "rtrim", "trim"] {
                state.number(usize::from(
                    arena.slots[definition].node.get(flag).is_some(),
                ));
            }
            let mut stack = vec![definition];
            while let Some(id) = stack.pop() {
                let node = &arena.slots[id].node;
                state.text(node.kind);
                state.text(node.text.as_deref().unwrap_or_default());
                state.text(arena.str_attr(id, "refname"));
                state.number(arena.slots[id].kids.len());
                stack.extend(arena.slots[id].kids.iter().rev());
            }
        }
        let mut names: Vec<&String> = nested.keys().collect();
        names.sort();
        for name in names {
            let mut keys: Vec<&String> = nested[name].iter().collect();
            keys.sort();
            keys.dedup();
            state.text(name);
            state.number(keys.len());
            keys.into_iter().for_each(|key| state.text(key));
        }
        let names_of: HashMap<usize, &str> =
            defs.iter().map(|(name, &id)| (id, name.as_str())).collect();
        let mut others: HashMap<usize, usize> = HashMap::new();
        state.number(pending.len());
        for &reference in pending {
            state.text(arena.str_attr(reference, "refname"));
            // Up to the nearest definition node, noting the places.
            let mut places = Vec::new();
            let mut at = reference;
            let mut owner = None;
            while let Some(parent) = arena.slots[at].parent {
                places.push(arena.slots[parent].kids.iter().position(|&kid| kid == at));
                if arena.slots[parent].node.kind == "substitution_definition" {
                    owner = Some(parent);
                    break;
                }
                at = parent;
            }
            match owner {
                None => state.text("text"),
                Some(owner) => {
                    match names_of.get(&owner) {
                        Some(name) => {
                            state.text("definition");
                            state.text(name);
                        }
                        None => {
                            let count = others.len();
                            state.text("other");
                            state.number(*others.entry(owner).or_insert(count));
                            state.number(usize::from(arena.is_attached(owner)));
                        }
                    }
                    state.number(places.len());
                    for place in places.iter().rev() {
                        state.number(place.unwrap_or(usize::MAX));
                    }
                }
            }
        }
        state.0.finalize()
    }

    fn number(&mut self, value: usize) {
        self.0.update(&(value as u64).to_le_bytes());
    }

    fn text(&mut self, value: &str) {
        self.number(value.len());
        self.0.update(value.as_bytes());
    }
}

/// A doctree the way docutils holds one: every node reachable by index and
/// knowing its parent, the way docutils' `node.parent` does — which
/// `Element.replace` never clears (`nodes.py:1101-1109`), so a node taken
/// out of its parent's children still points at it. Substitutions
/// (`references.py:674-764`) depends on that: it expands references inside
/// copies and definitions that are no longer in the tree, and locates
/// references by ancestors they have left. So do IndirectHyperlinks, which
/// goes on resolving targets a `problematic` replaced (`:244-265`), and
/// DanglingReferences, which walks into a replaced reference's children
/// (`Node.walk`, `nodes.py:193-195`). Every walk is by explicit stack.
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
        self.get_str(id, key).unwrap_or_default()
    }

    fn get_str(&self, id: usize, key: &'static str) -> Option<&str> {
        str_value(&self.slots[id].node, key)
    }

    fn has(&self, id: usize, key: &'static str) -> bool {
        self.slots[id].node.get(key).is_some()
    }

    fn set(&mut self, id: usize, key: &'static str, value: String) {
        self.slots[id].node.set(key, AttrValue::Str(value));
    }

    fn remove(&mut self, id: usize, key: &str) {
        self.slots[id].node.remove(key);
    }

    fn kind(&self, id: usize) -> &'static str {
        self.slots[id].node.kind
    }

    /// Where docutils locates a substitution, hyperlink or footnote
    /// reference (or an embedded alias's target): `get_source_line`
    /// (`docutils/utils/__init__.py:645-654`) walks up to the first
    /// ancestor carrying a line, the inliner stamping none on the reference
    /// or on the `reference` a `|name|_` wraps it in — a paragraph, a
    /// definition (its marker line), a table cell's paragraph; a section
    /// title is unstamped too (`new_subsection`, `states.py:499-503`), so
    /// there it is the section's line. Elsewhere the enclosing element's
    /// span line stands in for docutils' `line` (ledgered where they differ:
    /// an attribution, a field name, a glossary term).
    fn location(&self, reference: usize) -> (u16, u32) {
        // The ancestors the rule reads: up to the first that is not a
        // `reference`, and that one's parent (a title's section).
        let mut ancestors = Vec::new();
        let mut at = reference;
        while let Some(parent) = self.slots[at].parent {
            ancestors.push(&self.slots[parent].node);
            at = parent;
            if self.slots[at].node.kind != kinds::REFERENCE {
                ancestors.extend(self.slots[at].parent.map(|up| &self.slots[up].node));
                break;
            }
        }
        let span = locating_ancestor(&self.slots[reference].node, &ancestors).span;
        (span.source, span.line)
    }

    /// `old.replace_self(new)` (`nodes.py:1110-1132`): the first new node,
    /// if an element, takes on `old`'s ids, classes, names and dupnames
    /// (`update_basic_atts`, `:850-869`, skipping values it has), and the new
    /// nodes take `old`'s place in its parent (`Element.replace`,
    /// `:1101-1109`), which becomes theirs. Does nothing, and answers
    /// `false`, when `old` has no parent or its parent no longer holds it,
    /// where docutils raises.
    fn replace_self(&mut self, old: usize, new: Vec<usize>) -> bool {
        let Some(parent) = self.slots[old].parent else {
            return false;
        };
        let Some(index) = self.slots[parent].kids.iter().position(|&kid| kid == old) else {
            return false;
        };
        if let Some(&first) = new.first() {
            if self.slots[first].node.kind != kinds::TEXT {
                let old_node = self.slots[old].node.shallow_copy();
                update_basic_atts(&mut self.slots[first].node, &old_node);
            }
        }
        for &node in &new {
            self.slots[node].parent = Some(parent);
        }
        self.slots[parent].kids.splice(index..=index, new);
        true
    }

    /// Whether `id`'s parent still holds it.
    fn is_attached(&self, id: usize) -> bool {
        self.slots[id]
            .parent
            .is_some_and(|parent| self.slots[parent].kids.contains(&id))
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

/// Every `target` below `root`, in document order.
fn collect_targets(root: &Node) -> Vec<NodePath> {
    collect_paths(root, |node| node.kind == kinds::TARGET)
}

/// `findall(condition)` from the root, as paths: every node below `root`
/// (`root` included) that `wanted` accepts, in document order — pre-order,
/// by an explicit stack of sibling cursors rather than recursion, building
/// a path only for the nodes it keeps.
pub(super) fn collect_paths(root: &Node, wanted: impl Fn(&Node) -> bool) -> Vec<NodePath> {
    let mut found = Vec::new();
    if wanted(root) {
        found.push(Vec::new());
    }
    // Each frame: a sibling list and the next index in it; `path` is the
    // path of the node whose children the top frame walks.
    let mut frames: Vec<(&[Node], usize)> = vec![(root.children.as_slice(), 0)];
    let mut path: NodePath = Vec::new();
    while let Some(frame) = frames.last_mut() {
        let (siblings, index) = (frame.0, frame.1);
        if index == siblings.len() {
            frames.pop();
            path.pop();
            continue;
        }
        frame.1 += 1;
        let node = &siblings[index];
        path.push(index);
        if wanted(node) {
            found.push(path.clone());
        }
        if node.children.is_empty() {
            path.pop();
        } else {
            frames.push((node.children.as_slice(), 0));
        }
    }
    found
}

/// The node whose line locates a message about `reference`, given its
/// ancestors nearest first ([`Arena::location`] gives the rule): the first
/// ancestor that is not a `reference` (or the last one, or the reference
/// itself without any), a section title standing in for its section.
fn locating_ancestor<'n>(reference: &'n Node, ancestors: &[&'n Node]) -> &'n Node {
    let Some(at) = ancestors
        .iter()
        .position(|node| node.kind != kinds::REFERENCE)
        .or_else(|| ancestors.len().checked_sub(1))
    else {
        return reference;
    };
    if ancestors[at].kind == kinds::TITLE {
        if let Some(section) = ancestors
            .get(at + 1)
            .filter(|node| node.kind == kinds::SECTION)
        {
            return section;
        }
    }
    ancestors[at]
}

/// [`Arena::location`] for the reference at `path` below `root`.
pub(super) fn location_at(root: &Node, path: &[usize]) -> (u16, u32) {
    let mut chain = vec![root];
    for &index in path {
        let Some(child) = chain.last().and_then(|node| node.children.get(index)) else {
            break;
        };
        chain.push(child);
    }
    let reference = chain.pop().unwrap_or(root);
    chain.reverse();
    let span = locating_ancestor(reference, &chain).span;
    (span.source, span.line)
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

/// `node.get(key)` as a string.
pub(super) fn str_value<'n>(node: &'n Node, key: &'static str) -> Option<&'n str> {
    match node.get(key) {
        Some(AttrValue::Str(value)) => Some(value),
        _ => None,
    }
}

/// `node.replace_self(new)` for the node at `path` (`nodes.py:1110-1132`):
/// `new` takes `old`'s place and its basic attributes
/// ([`update_basic_atts`]).
pub(super) fn replace_at(root: &mut Node, path: &[usize], mut new: Node) {
    let Some((&index, parent)) = path.split_last() else {
        return;
    };
    let Some(old) = node_at_mut(root, parent).and_then(|parent| parent.children.get_mut(index))
    else {
        return;
    };
    update_basic_atts(&mut new, old);
    *old = new;
}

/// `AnonymousHyperlinks` (`docutils/transforms/references.py:98-160`,
/// priority 440): the anonymous references (`` `x`__ ``, `x__`, `|x|__`)
/// and the anonymous targets (`__ uri`, `.. __: uri`), each in document
/// order (`findall`, `:127-132`), are paired one to one. (No anonymous
/// reference sits in a substitution definition, or in a copy of one: the
/// parse refuses them there, `Anonymous references are not supported in a
/// substitution definition.`, probed.)
///
/// * Counts that differ give one ERROR, `Anonymous hyperlink mismatch: %s
///   references but %s targets.\nSee "backrefs" attribute for IDs.`, with
///   no node to locate it by ([`TransformCtx::end_of_parse_message`],
///   research §9.3); its id is spent, and every anonymous reference is
///   replaced by a `problematic` pointing at it, each with the next id
///   (`:133-145`). The message stays out of the tree.
/// * Otherwise each reference without a `refid`/`refuri` of its own takes
///   its target's `refuri`; or, from a target PropagateTargets emptied,
///   the node its id moved to (`document.ids[target['refid']]`, `:155-157`)
///   — that node's `refuri`, else its first id; or the target's first id
///   (`:146-160`). A reference given a `refid` is noted in
///   `document.refids` (`:159`), where IndirectHyperlinks finds it again:
///   the walk-built lists do that ([`super::DocumentLists::refids`]).
pub(super) fn anonymous_hyperlinks(ctx: &mut TransformCtx) {
    let anonymous = |kind: &'static str| {
        move |node: &Node| node.kind == kind && node.get("anonymous").is_some()
    };
    let references = collect_paths(&ctx.tree.root, anonymous(kinds::REFERENCE));
    let targets = collect_paths(&ctx.tree.root, anonymous(kinds::TARGET));
    if references.is_empty() && targets.is_empty() {
        return;
    }
    if references.len() != targets.len() {
        let text = format!(
            "Anonymous hyperlink mismatch: {} references but {} targets.\n\
             See \"backrefs\" attribute for IDs.",
            references.len(),
            targets.len()
        );
        let message = ctx.end_of_parse_message(messages::ERROR, &text);
        ctx.reporter.report(&message);
        let message_id = ctx.ids.allocate_auto_id();
        let mut problematics = Vec::with_capacity(references.len());
        for path in &references {
            let reference = node_at(&ctx.tree.root, path).expect("a path just collected");
            let mut problematic = problematic_for(reference, message_id.clone());
            problematic.attrs.ids.push(ctx.ids.allocate_auto_id());
            problematics.push(problematic);
        }
        // The ids go out in document order; the replacements are made last
        // first, so that no path could run through a node already replaced
        // (none does: no anonymous reference holds another).
        for (path, problematic) in references.iter().zip(problematics).rev() {
            replace_at(&mut ctx.tree.root, path, problematic);
        }
        return;
    }
    let (tree, lists) = ctx.tree_and_lists();
    let mut links: Vec<(&NodePath, &'static str, String)> = Vec::new();
    for (reference_path, target_path) in references.iter().zip(&targets) {
        let (Some(reference), Some(mut target)) = (
            node_at(&tree.root, reference_path),
            node_at(&tree.root, target_path),
        ) else {
            continue;
        };
        if reference.get("refid").is_some() || reference.get("refuri").is_some() {
            continue;
        }
        if target.get("refuri").is_none() && target.attrs.ids.is_empty() {
            // A propagated target: the node carrying the id it handed on.
            // (Upstream raises `KeyError` where there is none.)
            let Some(moved) = str_value(target, "refid")
                .and_then(|refid| lists.ids.get(refid))
                .and_then(|path| node_at(&tree.root, path))
            else {
                continue;
            };
            target = moved;
        }
        if let Some(refuri) = str_value(target, "refuri") {
            links.push((reference_path, "refuri", refuri.to_string()));
        } else if let Some(id) = target.attrs.ids.first() {
            links.push((reference_path, "refid", id.clone()));
        }
    }
    for (path, key, value) in links {
        if let Some(reference) = node_at_mut(&mut tree.root, path) {
            reference.set(key, AttrValue::Str(value));
        }
    }
}

/// `IndirectHyperlinks` (`docutils/transforms/references.py:163-338`,
/// priority 460): every indirect target — a target with a `refname`
/// (`.. _a: b_`, `__ b_`, an embedded alias's `` `a <b_>`_ ``), in
/// `document.indirect_targets` order — is resolved unless it already is,
/// then hands what it resolved to on to the references naming it
/// (`:216-220`). See [`Links`] for the two steps and their errors.
pub(super) fn indirect_hyperlinks(ctx: &mut TransformCtx) {
    let lists = ctx.lists();
    if lists.indirect_targets.is_empty() {
        return;
    }
    let indirect_paths = lists.indirect_targets.clone();
    let refname_paths = lists.refnames.clone();
    let refid_paths = lists.refids.clone();
    let id_paths = lists.ids.clone();
    let root = std::mem::replace(&mut ctx.tree.root, Node::elem(kinds::DOCUMENT, Span::ZERO));
    let arena = Arena::new(root);
    let at = |paths: Vec<NodePath>| -> Vec<usize> {
        paths
            .iter()
            .filter_map(|path| arena.at_path(path))
            .collect()
    };
    let indirect = at(indirect_paths);
    let refnames = refname_paths
        .into_iter()
        .map(|(name, paths)| (name, at(paths)))
        .collect();
    let refids = refid_paths
        .into_iter()
        .map(|(id, paths)| (id, at(paths)))
        .collect();
    let ids = id_paths
        .into_iter()
        .filter_map(|(id, path)| Some((id, arena.at_path(&path)?)))
        .collect();
    let mut links = Links {
        arena,
        refnames,
        refids,
        ids,
        resolved: HashSet::new(),
        multiply_indirect: HashSet::new(),
    };
    for target in indirect {
        if !links.resolved.contains(&target) {
            links.resolve_indirect_target(ctx, target);
        }
        links.resolve_indirect_references(target);
    }
    ctx.tree.root = links.arena.into_tree();
}

/// IndirectHyperlinks' state: the tree as docutils holds it (an [`Arena`]:
/// a target or reference a `problematic` replaced keeps being resolved and
/// read, as in docutils), the `document` lists it reads and adds to
/// (`refnames`, `refids` — which `note_refid` grows — and `ids`), and the
/// two Python attributes it sets on nodes, `resolved` and
/// `multiply_indirect`. Both of docutils' recursions run on explicit
/// stacks, so no chain of targets is too long.
struct Links {
    arena: Arena,
    refnames: HashMap<String, Vec<usize>>,
    refids: HashMap<String, Vec<usize>>,
    ids: HashMap<String, usize>,
    resolved: HashSet<usize>,
    multiply_indirect: HashSet<usize>,
}

/// One step of `resolve_indirect_target`'s recursion, on an explicit stack.
enum Resolve {
    /// Look the target's `refname` up and, when it names an unresolved
    /// indirect target, resolve that one first.
    Enter(usize),
    /// Take over what the named node resolved to.
    Finish {
        target: usize,
        reftarget_id: String,
        reftarget: usize,
        had_refname: bool,
        recursed: bool,
    },
}

/// A target whose references `resolve_indirect_references` is rewriting:
/// the attribute and value they take, whether each is noted in `refids`,
/// and the references still to visit — `(node, found by id)`, those named
/// by the target's names first, then those pointing at its ids.
struct Rewrite {
    attname: &'static str,
    attval: String,
    note: bool,
    references: Vec<(usize, bool)>,
    next: usize,
}

impl Links {
    /// `resolve_indirect_target` (`references.py:222-265`): the node the
    /// target's `refname` names (`document.nameids`, then `document.ids`)
    /// — an unresolved indirect target resolved first, with the target
    /// marked `multiply_indirect` meanwhile, so meeting it again on the way
    /// is a circular reference (`:240-249`) — gives the target its
    /// `refuri` (dropping any `refid`), its `refid`, or, when it has
    /// neither but carries ids, its own id as `refid` (`:250-262`; each
    /// `refid` noted in `refids`); the `refname` goes (`:263-265`).
    /// Errors: an unknown or duplicate name, or a node with nothing to
    /// point at ([`Self::nonexistent_indirect_target`]), and the circle
    /// ([`Self::indirect_target_error`]). Where upstream raises
    /// `KeyError` — a target with neither `refname` nor `refid` (none
    /// reaches here), an id no node in the tree carries (docutils' `ids`
    /// still holds a node a transform took out, such as an inline target in
    /// a circular substitution definition; the walk-built one does not) —
    /// the target is left as it is.
    fn resolve_indirect_target(&mut self, ctx: &mut TransformCtx, start: usize) {
        let mut stack = vec![Resolve::Enter(start)];
        while let Some(step) = stack.pop() {
            match step {
                Resolve::Enter(target) => {
                    let refname = self.arena.get_str(target, "refname").map(str::to_string);
                    let reftarget_id = match &refname {
                        None => match self.arena.get_str(target, "refid") {
                            Some(refid) => refid.to_string(),
                            None => continue,
                        },
                        Some(refname) => match ctx.ids.name_id(refname) {
                            Some(Some(id)) if !id.is_empty() => id.to_string(),
                            // The unknown-reference resolvers come first
                            // (`:230-235`); Sphinx registers none.
                            _ => {
                                self.nonexistent_indirect_target(ctx, target);
                                continue;
                            }
                        },
                    };
                    let Some(&reftarget) = self.ids.get(&reftarget_id) else {
                        continue;
                    };
                    let recurse = self.arena.kind(reftarget) == kinds::TARGET
                        && !self.resolved.contains(&reftarget)
                        && self.arena.has(reftarget, "refname");
                    if recurse && self.multiply_indirect.contains(&target) {
                        self.indirect_target_error(ctx, target, "forming a circular reference");
                        continue;
                    }
                    if recurse {
                        self.multiply_indirect.insert(target);
                    }
                    stack.push(Resolve::Finish {
                        target,
                        reftarget_id,
                        reftarget,
                        had_refname: refname.is_some(),
                        recursed: recurse,
                    });
                    if recurse {
                        stack.push(Resolve::Enter(reftarget));
                    }
                }
                Resolve::Finish {
                    target,
                    reftarget_id,
                    reftarget,
                    had_refname,
                    recursed,
                } => {
                    if recursed {
                        self.multiply_indirect.remove(&target);
                    }
                    if let Some(refuri) = self.arena.get_str(reftarget, "refuri") {
                        let refuri = refuri.to_string();
                        self.arena.set(target, "refuri", refuri);
                        self.arena.remove(target, "refid");
                    } else if let Some(refid) = self.arena.get_str(reftarget, "refid") {
                        let refid = refid.to_string();
                        self.arena.set(target, "refid", refid);
                        self.note_refid(target);
                    } else if !self.arena.slots[reftarget].node.attrs.ids.is_empty() {
                        self.arena.set(target, "refid", reftarget_id);
                        self.note_refid(target);
                    } else {
                        self.nonexistent_indirect_target(ctx, target);
                        continue;
                    }
                    if had_refname {
                        self.arena.remove(target, "refname");
                    }
                    self.resolved.insert(target);
                }
            }
        }
    }

    /// `nonexistent_indirect_target` (`references.py:267-272`).
    fn nonexistent_indirect_target(&mut self, ctx: &mut TransformCtx, target: usize) {
        let refname = self.arena.get_str(target, "refname").unwrap_or_default();
        let explanation = if ctx.ids.name_id(refname).is_some() {
            "which is a duplicate, and cannot be used as a unique reference"
        } else {
            "which does not exist"
        };
        self.indirect_target_error(ctx, target, explanation);
    }

    /// `indirect_target_error` (`references.py:277-298`): ERROR `Indirect
    /// hyperlink target %s refers to target "%s", %s.`, naming the target
    /// `"<first name>" (id="<first id>")` (either part only when it has
    /// one), at the target (`base_node=target`: an explicit target's own
    /// line, an inline one's nearest stamped ancestor's); its id is spent,
    /// and every node naming the target or pointing at one of its ids —
    /// references, and indirect or propagated targets too — is replaced by
    /// a `problematic` pointing at it, each with the next id. The target
    /// counts as resolved. A node docutils would replace a second time (out
    /// of its parent by now, where `parent.index` raises) is left alone.
    fn indirect_target_error(&mut self, ctx: &mut TransformCtx, target: usize, explanation: &str) {
        let node = &self.arena.slots[target].node;
        let mut naming = String::new();
        if let Some(name) = node.attrs.names.first() {
            naming = format!("\"{name}\" ");
        }
        let mut references: Vec<usize> = Vec::new();
        for name in &node.attrs.names {
            references.extend(self.refnames.get(name).into_iter().flatten());
        }
        for id in &node.attrs.ids {
            references.extend(self.refids.get(id).into_iter().flatten());
        }
        if let Some(id) = node.attrs.ids.first() {
            naming.push_str(&format!("(id=\"{id}\")"));
        }
        let refname = str_value(node, "refname").unwrap_or_default();
        let text = format!(
            "Indirect hyperlink target {naming} refers to target \"{refname}\", {explanation}."
        );
        let (source, line) = self.target_location(target);
        ctx.reporter
            .report(&ctx.message(messages::ERROR, &text, source, Some(line)));
        let message_id = ctx.ids.allocate_auto_id();
        let mut seen = HashSet::new();
        for reference in references {
            if !seen.insert(reference) {
                continue; // `utils.uniq`
            }
            let mut problematic =
                problematic_for(&self.arena.slots[reference].node, message_id.clone());
            problematic.attrs.ids.push(ctx.ids.allocate_auto_id());
            let problematic = self.arena.adopt(problematic, None);
            self.arena.replace_self(reference, vec![problematic]);
        }
        self.resolved.insert(target);
    }

    /// Where docutils locates a message about a target: its own line —
    /// `add_target` stamps an explicit target's (`states.py:2121`) — or,
    /// for the target an embedded alias puts inside a paragraph, which has
    /// none, the nearest stamped ancestor's (`get_source_line`).
    fn target_location(&self, target: usize) -> (u16, u32) {
        let inline = self.arena.slots[target]
            .parent
            .is_some_and(|parent| is_text_element(self.arena.kind(parent)));
        if inline {
            self.arena.location(target)
        } else {
            let span = self.arena.slots[target].node.span;
            (span.source, span.line)
        }
    }

    /// `resolve_indirect_references` (`references.py:300-338`): a target
    /// with a `refid` (else a `refuri`; else nothing to hand on) gives it
    /// to every unresolved node named by its names (`refnames`, dropping
    /// their `refname`) and then pointing at its ids (`refids`, dropping
    /// their `refid`), marking each resolved and noting each new `refid`;
    /// a target among them hands it on in turn, before the next node.
    /// Each target's list is read when it is reached; what `note_refid`
    /// adds to a list meanwhile is always resolved already, so reading it
    /// whole up front skips the same nodes.
    fn resolve_indirect_references(&mut self, start: usize) {
        let mut stack: Vec<Rewrite> = self.rewrite(start).into_iter().collect();
        while let Some(rewrite) = stack.last_mut() {
            let Some(&(node, by_id)) = rewrite.references.get(rewrite.next) else {
                stack.pop();
                continue;
            };
            rewrite.next += 1;
            if !self.resolved.insert(node) {
                continue;
            }
            let (attname, attval, note) = (rewrite.attname, rewrite.attval.clone(), rewrite.note);
            self.arena
                .remove(node, if by_id { "refid" } else { "refname" });
            self.arena.set(node, attname, attval);
            if note {
                self.note_refid(node);
            }
            if self.arena.kind(node) == kinds::TARGET {
                stack.extend(self.rewrite(node));
            }
        }
    }

    /// The [`Rewrite`] `target` starts, if it has anything to hand on.
    fn rewrite(&self, target: usize) -> Option<Rewrite> {
        let (attname, note) = if self.arena.has(target, "refid") {
            ("refid", true)
        } else if self.arena.has(target, "refuri") {
            ("refuri", false)
        } else {
            return None;
        };
        let attval = self.arena.get_str(target, attname)?.to_string();
        let attrs = &self.arena.slots[target].node.attrs;
        let mut references: Vec<(usize, bool)> = Vec::new();
        for name in &attrs.names {
            let named = self.refnames.get(name).into_iter().flatten();
            references.extend(named.map(|&node| (node, false)));
        }
        for id in &attrs.ids {
            let pointing = self.refids.get(id).into_iter().flatten();
            references.extend(pointing.map(|&node| (node, true)));
        }
        Some(Rewrite {
            attname,
            attval,
            note,
            references,
            next: 0,
        })
    }

    /// `document.note_refid(node)` (`nodes.py:2012-2013`).
    fn note_refid(&mut self, node: usize) {
        if let Some(refid) = self.arena.get_str(node, "refid") {
            let refid = refid.to_string();
            self.refids.entry(refid).or_default().push(node);
        }
    }
}

/// Whether a node in `document.refnames` is still to be resolved by the
/// transforms after IndirectHyperlinks: it carries its `refname` and is not
/// a target — the only targets that still carry one are those
/// IndirectHyperlinks failed, which it marked resolved (`references.py:
/// 298`) — nor a footnote reference Footnotes (620) resolved without
/// taking its `refname` away ([`footnote_resolved_by_number`]).
fn awaits_resolution(node: &Node) -> bool {
    node.kind != kinds::TARGET
        && node.get("refname").is_some()
        && !footnote_resolved_by_number(node)
}

/// A labelled auto-numbered footnote reference (`[#nope]_`) that no
/// footnote carries the label of, which Footnotes numbered with the next
/// unlabelled footnote's number instead: `number_footnote_references`
/// gives it that footnote's `refid` and marks it resolved, but leaves its
/// `refname` (`references.py:536-569`, probed: `tx_footnotes.
/// unmatched_label_takes_a_number`). The one node that carries both, so
/// both mark it — and the transforms after Footnotes skip it, resolved.
fn footnote_resolved_by_number(node: &Node) -> bool {
    node.kind == kinds::FOOTNOTE_REFERENCE
        && node.get("refname").is_some()
        && node.get("refid").is_some()
}

/// `ExternalTargets` (`docutils/transforms/references.py:340-373`,
/// priority 640): for each target with a `refuri`, in document order, every
/// unresolved node named by one of its names (`document.refnames`: hyperlink,
/// footnote and citation references) drops its `refname` and takes the
/// `refuri`.
pub(super) fn external_targets(ctx: &mut TransformCtx) {
    let (tree, lists) = ctx.tree_and_lists();
    let mut links: Vec<(&NodePath, String)> = Vec::new();
    for path in collect_targets(&tree.root) {
        let Some(target) = node_at(&tree.root, &path) else {
            continue;
        };
        let Some(refuri) = str_value(target, "refuri") else {
            continue;
        };
        for name in &target.attrs.names {
            for reference in lists.refnames.get(name).into_iter().flatten() {
                links.push((reference, refuri.to_string()));
            }
        }
    }
    for (path, refuri) in links {
        if let Some(node) = node_at_mut(&mut tree.root, path).filter(|node| awaits_resolution(node))
        {
            node.remove("refname");
            node.set("refuri", AttrValue::Str(refuri));
        }
    }
}

/// `InternalTargets` (`docutils/transforms/references.py:376-411`, priority
/// 660): for each target with neither `refuri` nor `refid` — an internal
/// target PropagateTargets left in place (at the end of its section, before
/// an invisible node, inside a paragraph: `` _`inline` ``) — every
/// unresolved node named by one of its names drops its `refname` and takes
/// `refid = document.nameids[name]`. (A name without an id would leave the
/// node unchanged but resolved; no target's name is one, see the module
/// docs.) References to a section, a propagated target or a directive's
/// `:name:` are not a target's: DanglingReferences resolves them.
pub(super) fn internal_targets(ctx: &mut TransformCtx) {
    let root = &ctx.tree.root;
    let mut names: Vec<String> = Vec::new();
    for path in collect_targets(root) {
        if let Some(target) = node_at(root, &path) {
            if target.get("refuri").is_none() && target.get("refid").is_none() {
                names.extend(target.attrs.names.iter().cloned());
            }
        }
    }
    if names.is_empty() {
        return;
    }
    let refids: Vec<Option<String>> = names
        .iter()
        .map(|name| ctx.ids.name_id(name).flatten().map(str::to_string))
        .collect();
    let (tree, lists) = ctx.tree_and_lists();
    let mut links: Vec<(&NodePath, &str)> = Vec::new();
    for (name, refid) in names.iter().zip(&refids) {
        let Some(refid) = refid.as_deref().filter(|refid| !refid.is_empty()) else {
            continue;
        };
        for reference in lists.refnames.get(name).into_iter().flatten() {
            links.push((reference, refid));
        }
    }
    for (path, refid) in links {
        if let Some(node) = node_at_mut(&mut tree.root, path).filter(|node| awaits_resolution(node))
        {
            node.remove("refname");
            node.set("refid", AttrValue::Str(refid.to_string()));
        }
    }
}

/// `SphinxDanglingReferences` (`sphinx/transforms/references.py:18-30`,
/// priority 850): docutils' `DanglingReferences` (`docutils/transforms/
/// references.py:878-990`) with the reporter's level raised to WARNING
/// meanwhile, so its INFO `Hyperlink target "%s" is not referenced.`
/// (`:900-919`, a message it neither keeps nor marks in the tree) never
/// prints — and is not made here.
///
/// The visitor walks the whole tree in document order (`document.walk`) —
/// substitution definitions included, and the children of a node it has
/// just replaced, which it reaches out of the tree — and stops at every
/// `reference` and `footnote_reference` still carrying a `refname`
/// (`:937-940`; `citation_reference` too upstream, but Sphinx's
/// CitationReferenceTransform (619) has replaced every one by then,
/// `sphinx/domains/citation.py:150-177`) and is not resolved — the one
/// node Footnotes resolved with its `refname` left is skipped
/// ([`footnote_resolved_by_number`]):
///
/// * a name `document.nameids` maps to an id: the `refname` gives way to
///   that `refid` (`:941-949`);
/// * otherwise ERROR at the reference (its nearest stamped ancestor) —
///   `Duplicate target name, cannot be used as a unique reference: "%s".`
///   for a name duplicated away, else `Unknown target name: "%s".`, with a
///   paragraph of hints when the name holds `<` or `>` (`:954-980`) — and
///   the reference is replaced by a `problematic` pointing at the message,
///   which spends the next id; the `problematic` takes the reference's own
///   first id if it has one (a footnote reference), else the next
///   (`:981-990`).
pub(super) fn dangling_references(ctx: &mut TransformCtx) {
    let dangling = |node: &Node| {
        matches!(node.kind, kinds::REFERENCE | kinds::FOOTNOTE_REFERENCE)
            && node.get("refname").is_some()
            && !footnote_resolved_by_number(node)
    };
    if collect_paths(&ctx.tree.root, dangling).is_empty() {
        return;
    }
    let root = std::mem::replace(&mut ctx.tree.root, Node::elem(kinds::DOCUMENT, Span::ZERO));
    let mut arena = Arena::new(root);
    let mut stack = vec![Arena::ROOT];
    while let Some(id) = stack.pop() {
        if dangling(&arena.slots[id].node) {
            visit_dangling_reference(ctx, &mut arena, id);
        }
        // `children[:]` after the visit: a replaced reference's own.
        stack.extend(arena.slots[id].kids.iter().rev());
    }
    ctx.tree.root = arena.into_tree();
}

/// `DanglingReferencesVisitor.visit_reference` (`references.py:937-990`)
/// for a reference that still carries its `refname`.
fn visit_dangling_reference(ctx: &mut TransformCtx, arena: &mut Arena, reference: usize) {
    let refname = arena.str_attr(reference, "refname").to_string();
    let (text, hint) = match ctx.ids.name_id(&refname) {
        Some(Some(id)) => {
            let id = id.to_string();
            arena.remove(reference, "refname");
            arena.set(reference, "refid", id);
            return;
        }
        Some(None) => (
            format!("Duplicate target name, cannot be used as a unique reference: \"{refname}\"."),
            None,
        ),
        None => (
            format!("Unknown target name: \"{refname}\"."),
            embedded_reference_hint(&refname),
        ),
    };
    let (source, line) = arena.location(reference);
    let mut message = ctx.message(messages::ERROR, &text, source, Some(line));
    if let Some(hint) = hint {
        message = messages::with_paragraph(message, &hint);
    }
    ctx.reporter.report(&message);
    let message_id = ctx.ids.allocate_auto_id();
    let mut problematic = problematic_for(&arena.slots[reference].node, message_id);
    if arena.slots[reference].node.attrs.ids.is_empty() {
        problematic.attrs.ids.push(ctx.ids.allocate_auto_id());
    }
    let problematic = arena.adopt(problematic, None);
    arena.replace_self(reference, vec![problematic]);
}

/// The hint DanglingReferences adds to an unknown name holding `<` or `>`
/// (`references.py:959-976`): a mistyped embedded URI or alias.
fn embedded_reference_hint(refname: &str) -> Option<String> {
    if !refname.contains(['<', '>']) {
        return None;
    }
    let mut hint = String::from("Did you want to embed a URI or alias?");
    if !refname.contains('<') {
        hint.push_str("\nOpening bracket missing.");
    } else if !refname.contains(" <") {
        hint.push_str("\nThe embedded reference must be preceded by whitespace.");
    }
    if !refname.contains('>') {
        hint.push_str("\nClosing bracket missing.");
    } else if !refname.ends_with('>') {
        hint.push_str("\nThe embedded reference must be the last text before the end string.");
    }
    if refname.contains("< ") || refname.contains(" >") {
        hint.push_str("\nWhitespace around the embedded reference is not allowed.");
    }
    Some(hint)
}

#[cfg(test)]
mod tests {
    use crate::doctree::ids::IdRegistry;
    use crate::doctree::{kinds, AttrValue, Doctree, Node, Span};
    use crate::rst::{ParseOptions, RegistryExport};
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

    /// A typo inside a definition placed after its use (review, fix round
    /// 3): expanding `|prod|` meets `|vresion|`, a name no definition has;
    /// docutils looks it up without a default (`normed[...]`,
    /// `references.py:726`) and the `KeyError` aborts the Sphinx build — a
    /// loud failure. The pass queues the nested reference instead, like any
    /// other, so it fails as docutils fails an undefined reference: the
    /// definition's own `|vresion|` first (at the definition's line), then
    /// the copy in the paragraph (at the paragraph's), each a `problematic`
    /// — and everything else, `|ver|` included, is expanded.
    #[test]
    fn a_nested_undefined_reference_is_reported_and_the_rest_expanded() {
        let (tree, records) = read_bounded(
            "Use |prod| and |ver|.\n\n.. |prod| replace:: Foo |vresion|\n\
             .. |ver| replace:: 1.0\n",
        );
        assert_eq!(
            records,
            [
                (Some(3), undefined("vresion")),
                (Some(1), undefined("vresion"))
            ]
        );
        assert_eq!(
            tree.root.pformat(),
            "<document source=\"<snippet>\">\n\
             \x20   <paragraph>\n\
             \x20       Use \n\
             \x20       Foo \n\
             \x20       <problematic ids=\"id4\" refid=\"id3\">\n\
             \x20           |vresion|\n\
             \x20        and \n\
             \x20       1.0\n\
             \x20       .\n\
             \x20   <substitution_definition names=\"prod\">\n\
             \x20       Foo \n\
             \x20       <problematic ids=\"id2\" refid=\"id1\">\n\
             \x20           |vresion|\n\
             \x20   <substitution_definition names=\"ver\">\n\
             \x20       1.0\n"
        );
    }

    /// [`read`] on a thread, given five seconds: a pass that never ends
    /// fails the test instead of hanging it. The thread has the default
    /// spawned-thread stack (2 MiB), so a pass recursing as deep as its
    /// input is long overflows it.
    fn read_bounded(source: impl Into<String>) -> (Doctree, Vec<(Option<u32>, String)>) {
        let source = source.into();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(read(&source));
        });
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the read pass did not terminate")
    }

    fn detected(line: u32, source: &str) -> (Option<u32>, String) {
        (
            Some(line),
            format!("Circular substitution definition detected:\n\n{source}"),
        )
    }

    fn referenced(line: u32, name: &str) -> (Option<u32>, String) {
        (
            Some(line),
            format!("Circular substitution definition referenced: \"{name}\"."),
        )
    }

    /// Two definition names differing only in case: `|A|` expands `A`, the
    /// definition it names exactly (`references.py:685-686`), but docutils
    /// files each nested reference under its case-folded name's definition
    /// (`normed[...]`, `:726-728`) — here `a`, whose list only ever gains
    /// `b` and `A` — so its circularity test never fires, no definition
    /// grows, and the expansion never ends (docutils 0.22.4 hangs: probed by
    /// the review, `timeout 20` exit 124; no oracle case can exist). From
    /// the third round of expansions on, the state the expansion depends on
    /// repeats; the backstop then ends each pending reference with the
    /// ordinary circular errors, in worklist order: the reference in `A`,
    /// then the one in `b` ("detected", each definition replaced), then the
    /// paragraph's ("referenced", at the reference it started from).
    #[test]
    fn a_case_folded_cycle_ends_where_docutils_never_does() {
        let (tree, records) = read_bounded(
            ".. |A| replace:: |b|\n.. |b| replace:: |A|\n.. |a| replace:: z\n\nSee |A|.\n",
        );
        assert_eq!(
            records,
            [
                detected(1, ".. |A| replace:: |b|"),
                detected(2, ".. |b| replace:: |A|"),
                referenced(5, "A"),
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

    /// The backstop's circular branch can meet a definition its own earlier
    /// "detected" message already replaced — where docutils' `parent.index(
    /// old)` raises (`nodes.py:1101-1103`). It stops there, as the expansion
    /// does: no third "detected" for `a`, and the paragraph's `|B|` stays.
    /// (A fuzz document docutils never finishes: no oracle case.)
    #[test]
    fn the_backstop_stops_at_a_replaced_definition_too() {
        let (tree, records) = read_bounded(
            ".. |B| replace:: |B|\n.. |b| replace:: x\n.. |a| replace:: |a| |d| |B|\n\
             .. |d| replace:: x |b|\n\nSee |B|.\n\nSee |a|.\n",
        );
        let a = detected(3, ".. |a| replace:: |a| |d| |B|");
        assert_eq!(
            records,
            [
                referenced(8, "a"),
                a.clone(),
                detected(1, ".. |B| replace:: |B|"),
                a
            ]
        );
        assert!(contains_kind(&tree.root, kinds::SUBSTITUTION_REFERENCE));
    }

    /// A definition referencing itself twice: its first reference replaces
    /// it with the "detected" message, and its second finds it again,
    /// already out of the tree — where docutils' `parent.index(old)` raises
    /// (`nodes.py:1101-1103`) and the Sphinx build aborts, having printed
    /// both messages (probed). The pass stops there too: the same two
    /// records, and the references it had not reached — the paragraph's —
    /// stay as they are.
    #[test]
    fn a_second_circular_error_in_a_replaced_definition_ends_the_expansion() {
        let (tree, records) = read_bounded(".. |a| replace:: |a| |a|\n\nSee |a|.\n");
        let detected = detected(1, ".. |a| replace:: |a| |a|");
        assert_eq!(records, [detected.clone(), detected]);
        assert_eq!(tree.root.children[0].kind, kinds::SYSTEM_MESSAGE);
        let paragraph = &tree.root.children[1];
        assert_eq!(paragraph.children[1].kind, kinds::SUBSTITUTION_REFERENCE);
    }

    /// Two more documents docutils aborts on at the same `parent.index(old)`
    /// (a definition replaced by its "detected" message, met again), one of
    /// them all lower case: expanding on past that point never ends, so the
    /// pass stops where Sphinx does — having printed exactly what Sphinx
    /// printed before its `ValueError` (probed).
    #[test]
    fn expansion_stops_where_sphinx_aborts_on_a_replaced_definition() {
        let (_, records) = read_bounded(
            ".. |c| replace:: |d|\n.. |b| replace:: |a|\n.. |d| replace:: |b| |a|\n\
             .. |a| replace:: |c| |d|\n",
        );
        let c = detected(1, ".. |c| replace:: |d|");
        assert_eq!(
            records,
            [detected(4, ".. |a| replace:: |c| |d|"), c.clone(), c]
        );

        let (_, records) = read_bounded(
            ".. |B| replace:: |a|\n.. |c| replace:: |b| |a|\n.. |A| replace:: |c|\n\
             .. |a| replace:: |A|\n",
        );
        let c = detected(2, ".. |c| replace:: |b| |a|");
        assert_eq!(records, [c.clone(), c]);
    }

    /// The paragraph's references' `refuri`s, and whether every target
    /// ended with `refuri` and no `refname`.
    fn resolved_uris(tree: &Doctree) -> (Vec<Option<String>>, bool) {
        let uri = |node: &Node| match node.get("refuri") {
            Some(AttrValue::Str(uri)) => Some(uri.clone()),
            _ => None,
        };
        let references = tree.root.children[0]
            .children
            .iter()
            .filter(|node| node.kind == kinds::REFERENCE)
            .map(uri)
            .collect();
        let targets_resolved = tree
            .root
            .children
            .iter()
            .filter(|node| node.kind == kinds::TARGET)
            .all(|target| uri(target).is_some() && target.get("refname").is_none());
        (references, targets_resolved)
    }

    /// IndirectHyperlinks resolves a chain of indirect targets by recursion
    /// (`resolve_indirect_target`, `references.py:236-246`), as deep as the
    /// chain is long — here the first target names the second, and so on,
    /// so resolving the first descends through all of them. The port walks
    /// the chain with an explicit stack: twenty thousand targets resolve on
    /// a 2 MiB thread. (CPython stops at its recursion limit; ledgered with
    /// the other Sphinx crashes the pass carries on through.) A paragraph
    /// separates each target from the next: ReorderConsecutiveTargetAndIndex
    /// Nodes (220) takes time quadratic in a run of adjacent targets
    /// (ledgered), which this test is not about.
    #[test]
    fn a_long_indirect_chain_resolves_without_recursion() {
        const N: usize = 20_000;
        let mut source = String::from("See `a0`_.\n\n");
        for k in 0..N {
            source.push_str(&format!(".. _a{k}: a{}_\n\nP.\n\n", k + 1));
        }
        source.push_str(&format!(".. _a{N}: https://x.example/\n"));
        let (tree, records) = read_bounded(source);
        assert_eq!(records, []);
        let (references, targets_resolved) = resolved_uris(&tree);
        assert_eq!(references, [Some("https://x.example/".to_string())]);
        assert!(targets_resolved);
    }

    /// The other recursion: `resolve_indirect_references` (`references.py:
    /// 301-338`) hands a resolved target's `refuri` on to every target
    /// naming it, and from each of those to the targets naming *it* — here
    /// each target names the one before, so resolving the first (to the
    /// external `a0`) rewrites the whole chain from the inside out.
    #[test]
    fn a_long_chain_of_referring_targets_rewrites_without_recursion() {
        const N: usize = 20_000;
        let mut source = format!("See `a{N}`_.\n\n");
        for k in 1..=N {
            source.push_str(&format!(".. _a{k}: a{}_\n\nP.\n\n", k - 1));
        }
        source.push_str(".. _a0: https://x.example/\n");
        let (tree, records) = read_bounded(source);
        assert_eq!(records, []);
        let (references, targets_resolved) = resolved_uris(&tree);
        assert_eq!(references, [Some("https://x.example/".to_string())]);
        assert!(targets_resolved);
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
            &mut RegistryExport::default(),
        );
        let figure = &tree.root.children[0];
        let kinds: Vec<&str> = figure.children.iter().map(|child| child.kind).collect();
        assert_eq!(kinds, [kinds::IMAGE, "caption"], "the target is gone");
        assert_eq!(figure.children[1].attrs.ids, ["t"]);
        assert_eq!(figure.children[1].attrs.names, ["t"]);
    }
}
