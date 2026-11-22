//! docutils-exact id/name normalization, and the parser's registration
//! calls over the document state.
//!
//! Algorithms ported from docutils 0.22.4 `nodes.py` (`make_id`,
//! `fully_normalize_name`, `whitespace_normalize_name`, and the
//! `document` registration methods: `set_id`, `note_implicit_target`,
//! `note_explicit_target` and the `note_*` calls that fill the node
//! registries) and Sphinx's `make_id`. The id and name bookkeeping they
//! drive, with the Sphinx settings `id_prefix=''`, `auto_id_prefix='id'`
//! (sphinx/environment overrides docutils' `'%'` default — auto ids are
//! `id1`, `id2`, …), is the document state's
//! [`IdAllocator`](crate::transforms::state::IdAllocator) and
//! [`NameTable`](crate::transforms::state::NameTable); the registries,
//! which hold nodes by identity ([`Uid`]), are its
//! [`Registries`](crate::transforms::state::Registries).

use unicode_normalization::UnicodeNormalization;

use super::messages;
use super::pending::PendingTransform;
use super::{AttrValue, Node, Uid};
use crate::transforms::state::{DocumentState, NodeView, PendingRecord};
use crate::utils::py_isspace;

/// docutils `_non_id_translate_digraphs` (applied after lowercasing).
fn translate_digraph(c: char) -> Option<&'static str> {
    Some(match c as u32 {
        223 => "sz", // ß
        230 => "ae", // æ
        339 => "oe", // œ
        568 => "db", // ȸ
        569 => "qp", // ȹ
        _ => return None,
    })
}

/// docutils `_non_id_translate` (single-char replacements).
fn translate_single(c: char) -> Option<char> {
    Some(match c as u32 {
        248 => 'o', // ø
        273 => 'd', // đ
        295 => 'h', // ħ
        305 => 'i', // ı
        322 => 'l', // ł
        359 => 't', // ŧ
        384 => 'b', // ƀ
        387 => 'b', // ƃ
        392 => 'c', // ƈ
        396 => 'd', // ƌ
        402 => 'f', // ƒ
        409 => 'k', // ƙ
        410 => 'l', // ƚ
        414 => 'n', // ƞ
        421 => 'p', // ƥ
        427 => 't', // ƫ
        429 => 't', // ƭ
        436 => 'y', // ƴ
        438 => 'z', // ƶ
        485 => 'g', // ǥ
        549 => 'z', // ȥ
        564 => 'l', // ȴ
        565 => 'n', // ȵ
        566 => 't', // ȶ
        567 => 'j', // ȷ
        572 => 'c', // ȼ
        575 => 's', // ȿ
        576 => 'z', // ɀ
        583 => 'e', // ɇ
        585 => 'j', // ɉ
        587 => 'q', // ɋ
        589 => 'r', // ɍ
        591 => 'y', // ɏ
        _ => return None,
    })
}

/// docutils `nodes.make_id`. Result grammar: `[a-z](-?[a-z0-9]+)*` or empty.
pub fn make_id(s: &str) -> String {
    // 1. lowercase FIRST (order is load-bearing: Ü -> ü -> NFKD u).
    let lowered = s.to_lowercase();
    // 2. digraph + single-char translate tables (disjoint key sets).
    let mut translated = String::with_capacity(lowered.len());
    for c in lowered.chars() {
        if let Some(d) = translate_digraph(c) {
            translated.push_str(d);
        } else if let Some(r) = translate_single(c) {
            translated.push(r);
        } else {
            translated.push(c);
        }
    }
    // 3. NFKD-normalize, drop remaining non-ASCII.
    let ascii: String = translated.nfkd().filter(char::is_ascii).collect();
    // 4. collapse whitespace runs (' '.join(s.split()) — Python's
    //    `str.split()`, i.e. [`py_isspace`] runs).
    let collapsed = py_split_join(&ascii);
    // 5. every [^a-z0-9]+ run -> single '-'.
    let mut out = String::with_capacity(collapsed.len());
    let mut in_run = false;
    for c in collapsed.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    // 6. strip leading [-0-9]+ and trailing -+ (ASCII-only by now).
    let bytes = out.as_bytes();
    let mut start = 0;
    while start < bytes.len() && (bytes[start] == b'-' || bytes[start].is_ascii_digit()) {
        start += 1;
    }
    let mut end = bytes.len();
    while end > start && bytes[end - 1] == b'-' {
        end -= 1;
    }
    out[start..end].to_string()
}

/// sphinx `util.nodes._make_id` (`:537-561`) — a fork of docutils' 0.16
/// `make_id` with two documented changes: capital letters survive, and `.`
/// and `_` count as identifier characters (so `envvar-HOME_A` stays
/// `envvar-HOME_A`, where docutils' would yield `envvar-home-a`). The
/// translate tables are the same ones [`make_id`] uses; the lowercasing
/// step is what sphinx drops, so this runs them on the raw string.
///
/// Result grammar: `[^-0-9._][a-zA-Z0-9._-]*` with no trailing `-`, or empty.
pub fn sphinx_make_id(s: &str) -> String {
    // 1. digraph + single-char translate tables (NO lowercasing first).
    let mut translated = String::with_capacity(s.len());
    for c in s.chars() {
        if let Some(d) = translate_digraph(c) {
            translated.push_str(d);
        } else if let Some(r) = translate_single(c) {
            translated.push(r);
        } else {
            translated.push(c);
        }
    }
    // 2. NFKD-normalize, drop remaining non-ASCII.
    let ascii: String = translated.nfkd().filter(char::is_ascii).collect();
    // 3. collapse whitespace runs (' '.join(s.split())).
    let collapsed = py_split_join(&ascii);
    // 4. every [^a-zA-Z0-9._]+ run -> single '-'.
    let mut out = String::with_capacity(collapsed.len());
    let mut in_run = false;
    for c in collapsed.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    // 5. `^[-0-9._]+|-+$` -> '' (both ends, one re.sub pass).
    let bytes = out.as_bytes();
    let mut start = 0;
    while start < bytes.len() && matches!(bytes[start], b'-' | b'.' | b'_' | b'0'..=b'9') {
        start += 1;
    }
    let mut end = bytes.len();
    while end > start && bytes[end - 1] == b'-' {
        end -= 1;
    }
    out[start..end].to_string()
}

/// Python `' '.join(s.split())`: strip the ends and collapse every run of
/// `str.isspace` characters to one space. `str.split()` splits on
/// [`py_isspace`], which admits `\x1c`-`\x1f` where Rust's
/// `split_whitespace` (Unicode White_Space) does not — so `a\x1fb` is the
/// name `a b` under docutils (`nodes.py:3044-3050`; probed: `.. _a\x1fb:`
/// yields `names="a\ b"`, a `Sec\x1fC` title `names="sec\ c"`).
fn py_split_join(s: &str) -> String {
    s.split(py_isspace)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// docutils `fully_normalize_name`: lowercase + collapse whitespace.
pub fn fully_normalize_name(s: &str) -> String {
    py_split_join(&s.to_lowercase())
}

/// docutils `whitespace_normalize_name`: collapse whitespace, keep case.
pub fn whitespace_normalize_name(s: &str) -> String {
    py_split_join(s)
}

/// A deferred "first node loses its name too" fixup: on a duplicate name,
/// docutils dupnames BOTH nodes, but the first one is already deep in the
/// tree — the parser applies these after parsing via
/// [`apply_dupname_fixups`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DupnameFixup {
    pub name: String,
    pub node_id: String,
}

// ----------------------------------------------------------------------
// The parser's registration calls: docutils' and Sphinx's `document`
// registration methods, over the document state (its id allocator and name
// table, `crate::transforms::state`) and the node the parser holds. The
// messages a duplicate name raises are built here and handed back for the
// caller to place.
// ----------------------------------------------------------------------

impl DocumentState {
    /// docutils `document.set_id(node)` (`nodes.py:1827-1875`) on a node
    /// the parser holds. A node that already carries ids registers them
    /// and keeps the last one (`set_id` returns its loop variable); a node
    /// without ids takes a new one from the allocator. Either way the ids
    /// are held by the node's identity, when it has one.
    pub(crate) fn set_id(&mut self, node: &mut Node) -> String {
        let holder = node.uid();
        if let Some(last) = node.attrs.ids.last().cloned() {
            for id in &node.attrs.ids {
                self.ids.register(id, holder);
            }
            return last;
        }
        let id = self.ids.allocate(node.kind, &node.attrs.names, holder);
        node.attrs.ids.push(id.clone());
        id
    }

    /// The node's identity: the [`Uid`] it carries, or a newly minted one
    /// it is stamped with. Every node a registry or a pending record
    /// holds has one.
    pub(crate) fn identify(&mut self, node: &mut Node) -> Uid {
        match node.uid() {
            Some(uid) => uid,
            None => {
                let uid = self.mint_uid();
                node.set_uid(uid);
                uid
            }
        }
    }

    /// docutils `note_refname(node)` (`nodes.py:2009-2010`):
    /// `refnames[node['refname']].append(node)`. A node without a
    /// `refname` is a caller error (docutils raises `KeyError`) and is
    /// not noted.
    pub(crate) fn note_refname(&mut self, node: &mut Node) {
        let Some(AttrValue::Str(refname)) = node.get("refname") else {
            debug_assert!(false, "note_refname on a {} without a refname", node.kind);
            return;
        };
        let refname = refname.clone();
        let uid = self.identify(node);
        self.registries.refnames.push(&refname, uid);
    }

    /// docutils `note_indirect_target(target)` (`nodes.py:2015-2018`): the
    /// target joins `indirect_targets` and, when it has a name, `refnames`
    /// under the name it refers to.
    pub(crate) fn note_indirect_target(&mut self, target: &mut Node) {
        let uid = self.identify(target);
        self.registries.indirect_targets.push(uid);
        if !target.attrs.names.is_empty() {
            self.note_refname(target);
        }
    }

    /// docutils `note_autofootnote(footnote)`: `set_id`, then
    /// `autofootnotes.append` (`nodes.py:2023-2025`).
    pub(crate) fn note_autofootnote(&mut self, footnote: &mut Node) {
        let uid = self.identify(footnote);
        self.set_id(footnote);
        self.registries.autofootnotes.push(uid);
    }

    /// docutils `note_autofootnote_ref(ref)` (`nodes.py:2027-2029`).
    pub(crate) fn note_autofootnote_ref(&mut self, reference: &mut Node) {
        let uid = self.identify(reference);
        self.set_id(reference);
        self.registries.autofootnote_refs.push(uid);
    }

    /// docutils `note_symbol_footnote(footnote)` (`nodes.py:2031-2033`).
    pub(crate) fn note_symbol_footnote(&mut self, footnote: &mut Node) {
        let uid = self.identify(footnote);
        self.set_id(footnote);
        self.registries.symbol_footnotes.push(uid);
    }

    /// docutils `note_symbol_footnote_ref(ref)` (`nodes.py:2035-2037`).
    pub(crate) fn note_symbol_footnote_ref(&mut self, reference: &mut Node) {
        let uid = self.identify(reference);
        self.set_id(reference);
        self.registries.symbol_footnote_refs.push(uid);
    }

    /// docutils `note_footnote(footnote)` (`nodes.py:2039-2041`): a
    /// manually numbered footnote.
    pub(crate) fn note_footnote(&mut self, footnote: &mut Node) {
        let uid = self.identify(footnote);
        self.set_id(footnote);
        self.registries.footnotes.push(uid);
    }

    /// docutils `note_footnote_ref(ref)` (`nodes.py:2043-2046`):
    /// `set_id`, `footnote_refs[refname].append`, `note_refname`. A
    /// `[#name]_` reference arrives here after `note_autofootnote_ref`,
    /// so its `set_id` registers the id the node already holds.
    pub(crate) fn note_footnote_ref(&mut self, reference: &mut Node) {
        let uid = self.identify(reference);
        self.set_id(reference);
        if let Some(AttrValue::Str(refname)) = reference.get("refname") {
            let refname = refname.clone();
            self.registries.footnote_refs.push(&refname, uid);
        }
        self.note_refname(reference);
    }

    /// docutils `note_citation(citation)` (`nodes.py:2048-2049`): no
    /// `set_id`; the caller's `note_explicit_target` gives the id.
    pub(crate) fn note_citation(&mut self, citation: &mut Node) {
        let uid = self.identify(citation);
        self.registries.citations.push(uid);
    }

    /// docutils `note_citation_ref(ref)` (`nodes.py:2051-2054`).
    pub(crate) fn note_citation_ref(&mut self, reference: &mut Node) {
        let uid = self.identify(reference);
        self.set_id(reference);
        if let Some(AttrValue::Str(refname)) = reference.get("refname") {
            let refname = refname.clone();
            self.registries.citation_refs.push(&refname, uid);
        }
        self.note_refname(reference);
    }

    /// The registry half of docutils `note_substitution_def(subdef,
    /// def_name, msgnode)` (`nodes.py:2056-2073`), for the whitespace-
    /// normalized `name`: the definition is the name's latest, and its
    /// fully normalized form maps to it. The duplicate-name error and the
    /// old definition's `dupname` stay with the caller, which holds the
    /// tree.
    pub(crate) fn note_substitution_def(&mut self, subdef: &mut Node, name: &str) {
        let uid = self.identify(subdef);
        self.registries
            .substitution_defs
            .push((name.to_string(), uid));
        self.registries
            .substitution_names
            .insert(fully_normalize_name(name), name.to_string());
    }

    /// docutils `note_pending(pending)` (`nodes.py:2081-2083`): queues the
    /// node's transform, in parse order, by the node's identity.
    pub(crate) fn note_pending(
        &mut self,
        node: &mut Node,
        transform: PendingTransform,
        order: u32,
    ) {
        let node = self.identify(node);
        self.pending.push(PendingRecord {
            transform,
            order,
            node,
        });
    }

    /// docutils `note_implicit_target(node, msgnode)` (`nodes.py:1996-2000`):
    /// [`Self::set_id`], then `set_name_id_map(..., explicit=False)`.
    /// Returns the messages the node's duplicate names raise, located at
    /// `line` of `source`. docutils appends them to `msgnode`; the caller
    /// places them there.
    pub(crate) fn note_implicit_target(
        &mut self,
        node: &mut Node,
        line: u32,
        source: &str,
    ) -> Vec<Node> {
        self.note_target(node, false, line, source)
    }

    /// docutils `note_explicit_target(node, msgnode)` (`nodes.py:2002-2007`):
    /// [`Self::note_implicit_target`] with the names bound as explicit.
    pub(crate) fn note_explicit_target(
        &mut self,
        node: &mut Node,
        line: u32,
        source: &str,
    ) -> Vec<Node> {
        self.note_target(node, true, line, source)
    }

    /// `set_id` plus `set_name_id_map`, applying each reported duplicate
    /// the way `set_duplicate_name_id` does: the node in hand loses the
    /// name at once, an earlier node (already placed in the parser's tree)
    /// through a [`DupnameFixup`], and the message is built here.
    fn note_target(
        &mut self,
        node: &mut Node,
        explicit: bool,
        line: u32,
        source: &str,
    ) -> Vec<Node> {
        let id = self.set_id(node);
        let duplicates = self.names.bind(NodeView::of(node), &id, explicit);
        let mut reported = Vec::with_capacity(duplicates.len());
        for duplicate in duplicates {
            if duplicate.dupname_new {
                dupname(node, &duplicate.name);
            }
            if let Some(node_id) = duplicate.dupname_old {
                self.parse.dupname_fixups.push(DupnameFixup {
                    name: duplicate.name.clone(),
                    node_id,
                });
            }
            let mut message =
                messages::system_message(duplicate.level, &duplicate.message, line, source);
            if duplicate.backrefs {
                message.attrs.backrefs.push(id.clone());
            }
            reported.push(message);
        }
        reported
    }

    /// sphinx `env.new_serialno('index')`: returns the current value and
    /// increments (serials start at 0 per document).
    pub(crate) fn new_index_serialno(&mut self) -> u32 {
        let serial = self.parse.index_serial;
        self.parse.index_serial += 1;
        serial
    }

    /// sphinx `util.nodes.make_id(env, document, prefix, term)` (`:610-637`),
    /// both prefix regimes:
    ///
    /// - non-empty prefix: candidate `_make_id(f'{prefix}-{term}')`, rejected
    ///   when it collapses to just the prefix, then serial fallback
    ///   `f'{prefix}-{env.new_serialno(prefix)}'` until the id is free;
    /// - EMPTY prefix (the py domain's object ids, `domains/python/
    ///   _object.py:420`): `idformat` becomes `(id_prefix or 'id') + '%s'`
    ///   with the Sphinx-pinned `id_prefix=''`, so the candidate is the bare
    ///   `_make_id(term)` — dots, underscores and capitals survive, making
    ///   `mymod.C.meth` its own node id — rejected when empty, and the
    ///   serial fallback is `id{n}` (`id0`, `id1`, …) with the counter still
    ///   keyed by the empty prefix.
    ///
    /// Sphinx does NOT register the result in `document.ids` here —
    /// `note_explicit_target` → `document.set_id` does, which is
    /// [`IdAllocator::register`](crate::transforms::state::IdAllocator::register).
    pub(crate) fn sphinx_make_id(&mut self, prefix: &str, term: &str) -> String {
        let mut node_id = if term.is_empty() {
            None
        } else if prefix.is_empty() {
            let candidate = sphinx_make_id(term);
            // `if not node_id: node_id = None` — empty term hash.
            (!candidate.is_empty()).then_some(candidate)
        } else {
            let candidate = sphinx_make_id(&format!("{prefix}-{term}"));
            // "*term* is not good to generate a node_id."
            (candidate != prefix).then_some(candidate)
        };
        loop {
            match &node_id {
                Some(id) if !self.ids.is_taken(id) => return id.clone(),
                _ => {}
            }
            let counter = self.parse.serialnos.entry(prefix.to_string()).or_insert(0);
            let serial = *counter;
            *counter += 1;
            node_id = Some(if prefix.is_empty() {
                format!("id{serial}")
            } else {
                format!("{prefix}-{serial}")
            });
        }
    }
}

/// docutils `dupname(node, name)`: the name moves from `names` to
/// `dupnames`. A name the node does not carry stays put (docutils
/// raises).
fn dupname(node: &mut Node, name: &str) {
    if let Some(pos) = node.attrs.names.iter().position(|n| n == name) {
        node.attrs.names.remove(pos);
        node.attrs.dupnames.push(name.to_string());
    }
}

/// Post-parse pass: move `name` from `names` to `dupnames` on the node
/// carrying `node_id` (the FIRST occurrence keeps its id, loses its name).
pub fn apply_dupname_fixups(root: &mut Node, fixups: &[DupnameFixup]) {
    if fixups.is_empty() {
        return;
    }
    for fixup in fixups {
        apply_one_fixup(root, fixup);
    }
}

fn apply_one_fixup(node: &mut Node, fixup: &DupnameFixup) -> bool {
    if node.attrs.ids.contains(&fixup.node_id) {
        dupname(node, &fixup.name);
        return true;
    }
    node.children
        .iter_mut()
        .any(|child| apply_one_fixup(child, fixup))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctree::{kinds, AttrValue, Node, Span};

    /// Python's `str.split()` (what every docutils name normalizer and
    /// `make_id` collapse with) splits on `str.isspace`, which admits the
    /// C0 separators `\x1c`-`\x1f`; Rust's `split_whitespace` does not.
    /// Probed on docutils 0.22.4 (panel fix round D): `.. _a\x1fb:` is the
    /// label `a b` with id `a-b`, and a `Sec\x1fC` title has
    /// `names="sec\ c"`.
    #[test]
    fn name_normalizers_split_on_python_whitespace() {
        assert_eq!(fully_normalize_name("a\x1fb"), "a b");
        assert_eq!(fully_normalize_name("Sec\x1fC"), "sec c");
        assert_eq!(fully_normalize_name("\x1f a \x1c\x1d\x1e b \x1f"), "a b");
        assert_eq!(whitespace_normalize_name("A\x1fB"), "A B");
        assert_eq!(whitespace_normalize_name("\x1fA  B\x1f"), "A B");
        assert_eq!(make_id("a\x1fb"), "a-b");
        assert_eq!(make_id("\x1fa\x1f"), "a");
        assert_eq!(sphinx_make_id("envvar-FOO\x1fBAR"), "envvar-FOO-BAR");
        assert_eq!(sphinx_make_id("\x1fA.b\x1f"), "A.b");
    }

    #[test]
    fn make_id_basics() {
        assert_eq!(make_id("My  Section    Title!"), "my-section-title");
        assert_eq!(make_id("Hello World!"), "hello-world");
        assert_eq!(make_id("1. Intro"), "intro");
        assert_eq!(make_id("2026 report"), "report");
        assert_eq!(make_id("Überblick"), "uberblick");
        assert_eq!(make_id("straße"), "strasze");
        assert_eq!(make_id("!!!"), "");
        assert_eq!(make_id("123"), "");
        assert_eq!(make_id("..."), "");
    }

    /// sphinx's fork keeps case and treats `.`/`_` as identifier
    /// characters, which is why `envvar-HOME_A` survives where docutils'
    /// `make_id` would flatten it to `envvar-home-a`.
    #[test]
    fn sphinx_make_id_keeps_case_dots_and_underscores() {
        assert_eq!(sphinx_make_id("envvar-HOME_A"), "envvar-HOME_A");
        assert_eq!(sphinx_make_id("confval-my_setting"), "confval-my_setting");
        assert_eq!(sphinx_make_id("a.b.C"), "a.b.C");
        // `[^a-zA-Z0-9._]+` collapses to a single '-'.
        assert_eq!(
            sphinx_make_id("cmdoption-myprog---verbose"),
            "cmdoption-myprog-verbose"
        );
        assert_eq!(
            sphinx_make_id("term-source directory"),
            "term-source-directory"
        );
        // `^[-0-9._]+` and `-+$` are stripped; docutils strips only `[-0-9]+`.
        assert_eq!(sphinx_make_id("._-1abc--"), "abc");
        assert_eq!(sphinx_make_id("Überblick"), "Uberblick");
        assert_eq!(sphinx_make_id("!!!"), "");
    }

    #[test]
    fn state_sphinx_make_id_falls_back_to_a_per_prefix_serial() {
        let mut state = DocumentState::default();
        assert_eq!(state.sphinx_make_id("envvar", "HOME"), "envvar-HOME");
        // Not registered until `IdAllocator::register`, so the same call
        // repeats.
        assert_eq!(state.sphinx_make_id("envvar", "HOME"), "envvar-HOME");
        state.ids.register("envvar-HOME", None);
        assert_eq!(state.sphinx_make_id("envvar", "HOME"), "envvar-0");
        state.ids.register("envvar-0", None);
        assert_eq!(state.sphinx_make_id("envvar", "HOME"), "envvar-1");
        // A term that collapses to the prefix itself is "not good to
        // generate a node_id" and goes straight to the serial; the counter
        // is per prefix.
        assert_eq!(state.sphinx_make_id("cmdoption", "!!!"), "cmdoption-0");
    }

    /// The empty-prefix regime the py domain's `make_id(env, doc, '',
    /// fullname)` calls hit (research spec [PY §1.5]): the candidate is the
    /// bare `_make_id(term)` — dots and capitals survive, so the id IS the
    /// fullname — and the serial fallback switches to `id{n}` starting at
    /// `id0` (probe `duplicate_functions`: second `dup` gets `id0`).
    #[test]
    fn state_sphinx_make_id_empty_prefix_keeps_fullname_and_serials_as_id_n() {
        let mut state = DocumentState::default();
        assert_eq!(state.sphinx_make_id("", "mymod.C.meth"), "mymod.C.meth");
        state.ids.register("mymod.C.meth", None);
        assert_eq!(state.sphinx_make_id("", "mymod.C.meth"), "id0");
        state.ids.register("id0", None);
        assert_eq!(state.sphinx_make_id("", "mymod.C.meth"), "id1");
        // A term whose hash is empty goes straight to the serial; the
        // counter is shared per prefix ('' here), not per term.
        state.ids.register("id1", None);
        assert_eq!(state.sphinx_make_id("", "!!!"), "id2");
        // Empty term likewise.
        state.ids.register("id2", None);
        assert_eq!(state.sphinx_make_id("", ""), "id3");
    }

    #[test]
    fn name_normalization() {
        assert_eq!(
            fully_normalize_name("My  Phrase   Target"),
            "my phrase target"
        );
        assert_eq!(fully_normalize_name("Hello World!"), "hello world!");
        assert_eq!(fully_normalize_name("Überblick"), "überblick");
        assert_eq!(whitespace_normalize_name("A  B"), "A B");
    }

    #[test]
    fn note_implicit_target_assigns_ids_and_handles_implicit_duplicates() {
        let mut state = DocumentState::default();
        let mut s1 = Node::elem(kinds::SECTION, Span::ZERO);
        s1.attrs.names.push("duplicate".into());
        assert!(state
            .note_implicit_target(&mut s1, 3, "<snippet>")
            .is_empty());
        assert_eq!(s1.attrs.ids, vec!["duplicate"]);

        let mut s2 = Node::elem(kinds::SECTION, Span::ZERO);
        s2.attrs.names.push("duplicate".into());
        let reported = state.note_implicit_target(&mut s2, 7, "<snippet>");
        let [msg] = reported.as_slice() else {
            panic!("one INFO, got {reported:?}");
        };
        assert_eq!(s2.attrs.ids, vec!["id1"]);
        assert!(s2.attrs.names.is_empty());
        assert_eq!(s2.attrs.dupnames, vec!["duplicate"]);
        assert_eq!(msg.get("type"), Some(&AttrValue::Str("INFO".into())));
        assert_eq!(msg.get("line"), Some(&AttrValue::Int(7)));
        assert_eq!(msg.attrs.backrefs, vec!["id1"]);

        // The FIRST node's fixup is deferred (it lives in the tree):
        let fixups = std::mem::take(&mut state.parse.dupname_fixups);
        assert_eq!(
            fixups,
            vec![DupnameFixup {
                name: "duplicate".into(),
                node_id: "duplicate".into()
            }]
        );
        let mut root = Node::elem(kinds::DOCUMENT, Span::ZERO);
        root.children.push(s1);
        apply_dupname_fixups(&mut root, &fixups);
        let s1 = &root.children[0];
        assert!(s1.attrs.names.is_empty());
        assert_eq!(s1.attrs.dupnames, vec!["duplicate"]);
        assert_eq!(s1.attrs.ids, vec!["duplicate"]); // keeps its id
    }

    #[test]
    fn note_implicit_target_gives_unmakeable_names_auto_ids() {
        let mut state = DocumentState::default();
        for (i, title) in ["!!!", "123", "..."].iter().enumerate() {
            let mut s = Node::elem(kinds::SECTION, Span::ZERO);
            s.attrs.names.push(fully_normalize_name(title));
            state.note_implicit_target(&mut s, 1, "<snippet>");
            assert_eq!(s.attrs.ids, vec![format!("id{}", i + 1)]);
            assert_eq!(s.attrs.names.len(), 1); // names kept, no collision
        }
    }

    /// docutils puts the new id in a duplicate's `backrefs` unless the
    /// node is a `target` with a `refuri` (`nodes.py:1974-1977`).
    #[test]
    fn explicit_duplicate_warning_backrefs_unless_a_target_has_a_refuri() {
        let target = |name: &str, refuri: Option<&str>| {
            let mut node = Node::elem(kinds::TARGET, Span::ZERO);
            node.attrs.names.push(name.into());
            if let Some(refuri) = refuri {
                node.set("refuri", AttrValue::Str(refuri.into()));
            }
            node
        };
        let mut state = DocumentState::default();
        let mut t1 = target("dup", Some("https://1/"));
        assert!(state
            .note_explicit_target(&mut t1, 1, "<snippet>")
            .is_empty());
        let mut t2 = target("dup", Some("https://2/"));
        let reported = state.note_explicit_target(&mut t2, 3, "<snippet>");
        let [msg] = reported.as_slice() else {
            panic!("one WARNING, got {reported:?}");
        };
        assert_eq!(msg.get("type"), Some(&AttrValue::Str("WARNING".into())));
        assert!(msg.attrs.backrefs.is_empty()); // external: no backrefs
        assert_eq!(t2.attrs.ids, vec!["id1"]);
        assert_eq!(t2.attrs.dupnames, vec!["dup"]);

        let mut state = DocumentState::default();
        let mut i1 = target("t", None);
        state.note_explicit_target(&mut i1, 1, "<snippet>");
        let mut i2 = target("t", None);
        let reported = state.note_explicit_target(&mut i2, 5, "<snippet>");
        let [msg] = reported.as_slice() else {
            panic!("one WARNING, got {reported:?}");
        };
        assert_eq!(msg.attrs.backrefs, vec!["id1"]); // internal: backrefs
    }

    /// docutils `set_id` on a node that already carries ids registers
    /// every one, allocates nothing and returns the last.
    #[test]
    fn set_id_on_a_node_with_ids_registers_them_and_returns_the_last() {
        let mut state = DocumentState::default();
        state.ids.enable_trace();
        let mut node = Node::elem(kinds::TARGET, Span::ZERO);
        node.attrs.ids = vec!["first".into(), "second".into()];
        assert_eq!(state.set_id(&mut node), "second");
        assert_eq!(node.attrs.ids, vec!["first", "second"]);
        assert!(state.ids.is_taken("first") && state.ids.is_taken("second"));
        assert!(state.ids.trace().is_empty());
        let mut fresh = Node::elem(kinds::PROBLEMATIC, Span::ZERO);
        assert_eq!(state.set_id(&mut fresh), "id1");
        assert_eq!(state.ids.trace().len(), 1);
    }
}
