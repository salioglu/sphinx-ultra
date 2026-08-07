//! docutils-exact id/name normalization and the document id registry.
//!
//! Algorithms ported from docutils 0.22.4 `nodes.py` (`make_id`,
//! `fully_normalize_name`, `whitespace_normalize_name`, `document.set_id`,
//! `set_name_id_map`/`set_duplicate_name_id`), with the Sphinx settings
//! `id_prefix=''`, `auto_id_prefix='id'` baked in (sphinx/environment
//! overrides docutils' `'%'` default — auto ids are `id1`, `id2`, …).

use std::collections::{HashMap, HashSet};

use unicode_normalization::UnicodeNormalization;

use super::messages;
use super::Node;

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
    // 4. collapse whitespace runs (' '.join(s.split())).
    let collapsed = ascii.split_whitespace().collect::<Vec<_>>().join(" ");
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

/// docutils `fully_normalize_name`: lowercase + collapse whitespace.
pub fn fully_normalize_name(s: &str) -> String {
    s.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// docutils `whitespace_normalize_name`: collapse whitespace, keep case.
pub fn whitespace_normalize_name(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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

/// Document-level id/name registry (docutils `document.ids`/`nameids`/
/// `id_counter` with Sphinx auto-id settings).
#[derive(Debug, Default)]
pub struct IdRegistry {
    ids: HashSet<String>,
    /// name -> Some(id) while unique, None once duplicated.
    nameids: HashMap<String, Option<String>>,
    /// per-prefix auto-id counters (only "id" in wave 1).
    id_counter: HashMap<&'static str, u64>,
    fixups: Vec<DupnameFixup>,
}

impl IdRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// docutils `document.set_id` (id_prefix='', auto_id_prefix='id'):
    /// first unregistered nonempty `make_id(name)` wins; otherwise `idN`.
    fn allocate_id(&mut self, names: &[String]) -> String {
        for name in names {
            let base = make_id(name);
            if !base.is_empty() && !self.ids.contains(&base) {
                self.ids.insert(base.clone());
                return base;
            }
        }
        loop {
            let counter = self.id_counter.entry("id").or_insert(0);
            *counter += 1;
            let id = format!("id{counter}");
            if !self.ids.contains(&id) {
                self.ids.insert(id.clone());
                return id;
            }
        }
    }

    fn register(
        &mut self,
        node: &mut Node,
        line: u32,
        source: &str,
        explicit: bool,
        backrefs_on_msg: bool,
    ) -> Option<Node> {
        let id = self.allocate_id(&node.attrs.names);
        node.attrs.ids.push(id.clone());

        let mut message = None;
        let names = node.attrs.names.clone();
        for name in names {
            match self.nameids.get(&name) {
                None => {
                    self.nameids.insert(name, Some(id.clone()));
                }
                Some(entry) => {
                    // Duplicate: dupname the NEW node now, queue the OLD one.
                    if let Some(old_id) = entry.clone() {
                        self.fixups.push(DupnameFixup {
                            name: name.clone(),
                            node_id: old_id,
                        });
                    }
                    self.nameids.insert(name.clone(), None);
                    let pos = node.attrs.names.iter().position(|n| *n == name);
                    if let Some(pos) = pos {
                        node.attrs.names.remove(pos);
                        node.attrs.dupnames.push(name.clone());
                    }
                    let (level, kind_word) = if explicit {
                        (messages::WARNING, "explicit")
                    } else {
                        (messages::INFO, "implicit")
                    };
                    let mut msg = messages::system_message(
                        level,
                        &format!("Duplicate {kind_word} target name: \"{name}\"."),
                        line,
                        source,
                    );
                    if backrefs_on_msg {
                        msg.attrs.backrefs.push(id.clone());
                    }
                    message = Some(msg);
                }
            }
        }
        message
    }

    /// Register an implicit target (section). On duplicate: INFO/1 message
    /// (placed by the caller inside the new section after its title), new
    /// node dupname'd immediately, old node queued for
    /// [`apply_dupname_fixups`].
    pub fn set_id_implicit(&mut self, node: &mut Node, line: u32, source: &str) -> Option<Node> {
        self.register(node, line, source, false, true)
    }

    /// Register an explicit target (`.. _name:` forms). On duplicate:
    /// WARNING/2; `backrefs` appear on the message only for internal targets
    /// (probe-verified: external/refuri duplicates carry no backrefs).
    pub fn set_id_explicit(
        &mut self,
        node: &mut Node,
        line: u32,
        source: &str,
        internal: bool,
    ) -> Option<Node> {
        self.register(node, line, source, true, internal)
    }

    /// Register an anonymous target: always an auto id, never a name.
    pub fn set_id_anonymous(&mut self, node: &mut Node) {
        let id = self.allocate_id(&[]);
        node.attrs.ids.push(id);
    }

    pub fn take_fixups(&mut self) -> Vec<DupnameFixup> {
        std::mem::take(&mut self.fixups)
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
        if let Some(pos) = node.attrs.names.iter().position(|n| *n == fixup.name) {
            node.attrs.names.remove(pos);
            node.attrs.dupnames.push(fixup.name.clone());
        }
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
    fn registry_assigns_ids_and_handles_implicit_duplicates() {
        let mut reg = IdRegistry::new();
        let mut s1 = Node::elem(kinds::SECTION, Span::ZERO);
        s1.attrs.names.push("duplicate".into());
        assert!(reg.set_id_implicit(&mut s1, 3, "<snippet>").is_none());
        assert_eq!(s1.attrs.ids, vec!["duplicate"]);

        let mut s2 = Node::elem(kinds::SECTION, Span::ZERO);
        s2.attrs.names.push("duplicate".into());
        let msg = reg
            .set_id_implicit(&mut s2, 7, "<snippet>")
            .expect("dup INFO");
        assert_eq!(s2.attrs.ids, vec!["id1"]);
        assert!(s2.attrs.names.is_empty());
        assert_eq!(s2.attrs.dupnames, vec!["duplicate"]);
        assert_eq!(msg.get("type"), Some(&AttrValue::Str("INFO".into())));
        assert_eq!(msg.get("line"), Some(&AttrValue::Int(7)));
        assert_eq!(msg.attrs.backrefs, vec!["id1"]);

        // The FIRST node's fixup is deferred (it lives in the tree):
        let fixups = reg.take_fixups();
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
    fn registry_auto_ids_for_unmakeable_names() {
        let mut reg = IdRegistry::new();
        for (i, title) in ["!!!", "123", "..."].iter().enumerate() {
            let mut s = Node::elem(kinds::SECTION, Span::ZERO);
            s.attrs.names.push(fully_normalize_name(title));
            reg.set_id_implicit(&mut s, 1, "<snippet>");
            assert_eq!(s.attrs.ids, vec![format!("id{}", i + 1)]);
            assert_eq!(s.attrs.names.len(), 1); // names kept, no collision
        }
    }

    #[test]
    fn explicit_duplicate_warning_backrefs_only_when_internal() {
        let mut reg = IdRegistry::new();
        let mut t1 = Node::elem(kinds::TARGET, Span::ZERO);
        t1.attrs.names.push("dup".into());
        assert!(reg
            .set_id_explicit(&mut t1, 1, "<snippet>", false)
            .is_none());

        let mut t2 = Node::elem(kinds::TARGET, Span::ZERO);
        t2.attrs.names.push("dup".into());
        let msg = reg
            .set_id_explicit(&mut t2, 3, "<snippet>", false)
            .expect("dup WARNING");
        assert_eq!(msg.get("type"), Some(&AttrValue::Str("WARNING".into())));
        assert!(msg.attrs.backrefs.is_empty()); // external: no backrefs
        assert_eq!(t2.attrs.ids, vec!["id1"]);
        assert_eq!(t2.attrs.dupnames, vec!["dup"]);

        let mut reg = IdRegistry::new();
        let mut i1 = Node::elem(kinds::TARGET, Span::ZERO);
        i1.attrs.names.push("t".into());
        reg.set_id_explicit(&mut i1, 1, "<snippet>", true);
        let mut i2 = Node::elem(kinds::TARGET, Span::ZERO);
        i2.attrs.names.push("t".into());
        let msg = reg
            .set_id_explicit(&mut i2, 5, "<snippet>", true)
            .expect("dup WARNING");
        assert_eq!(msg.attrs.backrefs, vec!["id1"]); // internal: backrefs
    }
}
