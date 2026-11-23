//! docutils' `get_source_line` over the transforms' arena ([DS §6.1,
//! §8.2]).
//!
//! A transform locates a message about a node where docutils locates the
//! node: `reporter.system_message(..., base_node=node)` takes
//! `utils.get_source_line(node)` (`SP/docutils/utils/__init__.py:645-654`),
//! the `(source, line)` of the first of the node and its ancestors that
//! has either, and Sphinx's `location=node` warnings walk the same way
//! (`get_node_location`, `SP/sphinx/util/logging.py:554-562`). docutils
//! keeps `source` and `line` on its node objects, where the parser stamps
//! every node with its span; a node's docutils location is derived from
//! the span by [DS §8.2]'s rules, as it stands once Sphinx's
//! `ApplySourceWorkaround` (010) has run, unless the parse recorded it
//! ([`DocumentState::loc_overrides`]).

use crate::doctree::{is_text_element, kinds, Span};
use crate::transforms::state::{DocLoc, DocumentState};
use crate::transforms::tree::{TxId, TxTree};

/// docutils `get_source_line(node)` for the node at `id`: the location of
/// the node, or of its nearest ancestor that has a source or a line, up
/// the parent links (a detached node's lead up its detached subtree).
/// `DocLoc::default()` when none has either; the reporter then locates the
/// message at the state machine's final position ([DS §6.2]).
pub fn get_source_line(tree: &TxTree, id: TxId, state: &DocumentState) -> DocLoc {
    let mut node = Some(id);
    while let Some(current) = node {
        let loc = own_location(tree, current, state);
        if loc.source.is_some() || loc.line.is_some() {
            return loc;
        }
        node = tree.parent(current);
    }
    DocLoc::default()
}

/// The kinds docutils never gives a location ([DS §6.1]'s census).
const UNLOCATED: &[&str] = &[
    kinds::DOCUMENT,
    kinds::ENTRY,
    kinds::ROW,
    kinds::TBODY,
    kinds::TGROUP,
    kinds::THEAD,
    kinds::COLSPEC,
    kinds::FIELD_BODY,
    kinds::LABEL,
    kinds::OPTION,
    kinds::OPTION_GROUP,
    kinds::DESCRIPTION,
];

/// docutils' `(node.source, node.line)` of the node at `id` after 010: the
/// location the parse recorded for it, if any; otherwise [DS §8.2]'s
/// rules:
///
/// 1. a node in inline position, a child of a `TextElement`, has none, as
///    has a Text node;
/// 2. so has a node of a kind docutils never locates ([`UNLOCATED`]);
/// 3. 010 gives a section's title its section's location
///    (`SP/sphinx/util/nodes.py:153-159`: a title without a source takes
///    its parent's), and a classifier its item's source and no line
///    (`:139-141`: docutils makes a classifier with a rawsource and
///    without a source);
/// 4. any other node has its span's source and line, and none when the
///    line is unknown (0).
fn own_location(tree: &TxTree, id: TxId, state: &DocumentState) -> DocLoc {
    let node = tree.node(id);
    if let Some(loc) = node.uid().and_then(|uid| state.loc_overrides.get(&uid)) {
        return *loc;
    }
    let parent = tree.parent(id);
    if parent.is_some_and(|parent| is_text_element(tree.node(parent).kind)) {
        return DocLoc::default();
    }
    match node.kind {
        kinds::TEXT => DocLoc::default(),
        kind if UNLOCATED.contains(&kind) => DocLoc::default(),
        kinds::TITLE => match parent {
            Some(section) if tree.node(section).kind == kinds::SECTION => {
                own_location(tree, section, state)
            }
            _ => span_location(node.span),
        },
        kinds::CLASSIFIER => DocLoc {
            source: parent.and_then(|item| own_location(tree, item, state).source),
            line: None,
        },
        _ => span_location(node.span),
    }
}

/// [DS §8.2] rule 4: the span's source and line; nothing for an unknown
/// line.
fn span_location(span: Span) -> DocLoc {
    if span.line == 0 {
        DocLoc::default()
    } else {
        DocLoc::at(span.source, span.line)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::doctree::{kinds, AttrValue};
    use crate::rst::{parse_rst_full, ParseOptions, ParseOutput};
    use crate::transforms::state::SourceRef;
    use crate::transforms::tree::TxNode;

    /// `node[key]` for a string attribute.
    fn attr<'a>(node: &'a TxNode, key: &str) -> Option<&'a str> {
        node.attrs
            .extra
            .iter()
            .find(|(name, _)| *name == key)
            .and_then(|(_, value)| match value {
                AttrValue::Str(value) => Some(value.as_str()),
                _ => None,
            })
    }

    /// What Substitutions (220) does to a defined reference: it is
    /// replaced by a deep copy of its definition's children.
    fn substitute(tree: &mut TxTree, state: &mut DocumentState) {
        let references = tree.findall(tree.root(), |node| {
            node.kind == kinds::SUBSTITUTION_REFERENCE
        });
        for reference in references {
            let refname = attr(tree.node(reference), "refname").expect("a refname");
            let (_, definition) = state
                .registries
                .substitution_defs
                .iter()
                .rev()
                .find(|(name, _)| name == refname)
                .expect("defined");
            let definition = tree.by_uid(*definition).expect("registered");
            let copy = tree.deepcopy(definition, state);
            let children = tree.node(copy).children.clone();
            tree.replace_self(reference, children);
        }
    }

    /// A location as Sphinx's warning stream prints it: `path:line`, or
    /// `path:` with no line.
    fn render(loc: DocLoc, sources: &[String]) -> String {
        let path = match loc.source {
            Some(SourceRef::Table(source)) => sources[usize::from(source)].as_str(),
            None => "",
        };
        match loc.line {
            Some(line) => format!("{path}:{line}"),
            None => format!("{path}:"),
        }
    }

    /// Where docutils locates a message about a node ([DS §6.1]): the
    /// read-state fixture's `state.location.*` cases, [DS §6.1]'s probe
    /// rows and the contexts T6 added, each a `bad_` reference that
    /// SphinxDanglingReferences reports as `Unknown target name: "bad"`.
    /// The locations it prints are the walk-up from each reference, in the
    /// order it reports them: the tree's pre-order once Substitutions
    /// (220) has copied the definition's reference in. An inline node has
    /// no location of its own, a paragraph has its first line, a section
    /// title its section's, a classifier its item's source and no line,
    /// and an image's `:target:` reference has the source only.
    #[test]
    fn location_rules_match_docutils() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/read_state_differential.json");
        let text = std::fs::read_to_string(path).expect("fixture readable");
        let fixture: Value = serde_json::from_str(&text).expect("fixture parses");
        let mut rows = 0;
        let mut mismatches = Vec::new();
        for case in fixture["cases"].as_array().expect("cases") {
            let name = case["name"].as_str().expect("name");
            if !name.starts_with("state.location.") {
                continue;
            }
            let expected: Vec<&str> = case["warnings"]
                .as_array()
                .expect("warnings")
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|line| {
                    line.strip_suffix(": ERROR: Unknown target name: \"bad\". [docutils]")
                })
                .collect();
            assert!(!expected.is_empty(), "{name}");
            let ParseOutput {
                doctree, mut state, ..
            } = parse_rst_full(
                case["rst"].as_str().expect("rst"),
                &ParseOptions {
                    source_path: "<SRCDIR>/index.rst".to_string(),
                    sphinx: true,
                    docname: "index".to_string(),
                    ..ParseOptions::default()
                },
            );
            let sources = doctree.sources.clone();
            let mut tree = TxTree::from_parse(doctree, &mut state);
            substitute(&mut tree, &mut state);
            let bad = |node: &TxNode| {
                node.kind == kinds::REFERENCE && attr(node, "refname") == Some("bad")
            };
            let ours: Vec<String> = tree
                .findall(tree.root(), bad)
                .into_iter()
                .map(|reference| render(get_source_line(&tree, reference, &state), &sources))
                .collect();
            if ours != expected {
                mismatches.push(format!("{name}: {ours:?}, docutils {expected:?}"));
            }
            rows += 1;
        }
        assert!(rows >= 27, "only {rows} location cases");
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
