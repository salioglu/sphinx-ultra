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
//! most nodes with their span. A node's location here is the one docutils'
//! parse leaves it ([`parse_location`]: [DS §8.2]'s rules over the span,
//! unless the parse recorded it, [`DocumentState::loc_overrides`]) as
//! Sphinx's `ApplySourceWorkaround` (010) completes it
//! ([`apply_source_workaround`]).

use crate::doctree::{is_text_element, kinds, Span};
use crate::transforms::state::{DocLoc, DocumentState, SourceRef};
use crate::transforms::tree::{TxId, TxTree};

/// docutils `get_source_line(node)` for the node at `id`: the location of
/// the node, or of its nearest ancestor that has a source or a line, up
/// the parent links (for a detached node, up its detached subtree).
/// `DocLoc::default()` when none has either; the reporter then locates the
/// message at the state machine's final position ([DS §6.2]). The test is
/// Python's `node.source or node.line`, so a node at line 0 with no source,
/// where 010 puts a node it finds no located ancestor for, has neither.
pub fn get_source_line(tree: &TxTree, id: TxId, state: &DocumentState) -> DocLoc {
    let mut node = Some(id);
    while let Some(current) = node {
        let loc = own_location(tree, current, state);
        if loc.source.is_some() || loc.line.is_some_and(|line| line != 0) {
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

/// docutils' `(node.source, node.line)` of the node at `id` once Sphinx's
/// ApplySourceWorkaround (010) has run: the location docutils' parse
/// leaves it, as 010 completes it. Unlike [`get_source_line`], the node's
/// own, as a transform reading `node.line` reads it (ClassAttribute's
/// `line=pending.line`).
pub fn own_location(tree: &TxTree, id: TxId, state: &DocumentState) -> DocLoc {
    let parsed = parse_location(tree, id, state);
    apply_source_workaround(tree, id, state, parsed)
}

/// Freezes the locations ApplySourceWorkaround (010) gives the nodes of
/// the subtree at `id`, ahead of a transform that moves it under other
/// ancestors. Sphinx applies 010 once, at priority 10, and a node keeps
/// what it got there wherever it moves; this model applies 010 at every
/// lookup ([`own_location`]), reading the ancestors the node has then. A
/// node 010 completed from an ancestor that does not move with it would
/// read its new ancestors instead: DocInfo (340) moves a field body's
/// content into a topic it makes, which has no location, so a rubric 010
/// gave the field's source and line 0 would have neither. Each node 010
/// completes takes the completed location as its override
/// ([`DocumentState::loc_overrides`]), which stays what 010 made it: an
/// override with a source is final, and one without, a rubric with no
/// located ancestor, completes to the same under ancestors that have no
/// location either, as DocInfo's have none.
pub fn freeze_workaround(tree: &mut TxTree, id: TxId, state: &mut DocumentState) {
    let completed: Vec<(TxId, DocLoc)> = tree
        .findall(id, |_| true)
        .into_iter()
        .filter_map(|node| {
            let parsed = parse_location(tree, node, state);
            let done = apply_source_workaround(tree, node, state, parsed);
            (done != parsed).then_some((node, done))
        })
        .collect();
    for (node, loc) in completed {
        let uid = tree.assign_uid(node, state);
        state.loc_overrides.insert(uid, loc);
    }
}

/// docutils' `(node.source, node.line)` of the node at `id` once the parse
/// has ended, before 010: the location the parse recorded for it, if any
/// ([`DocumentState::loc_overrides`]); otherwise [DS §8.2]'s rules:
///
/// 1. a node in inline position, a child of a `TextElement`, has none, as
///    has a Text node. An `image` is the exception: docutils' Image
///    directive stamps every image it makes with its source and line
///    (`SP/docutils/parsers/rst/directives/images.py:99-101`), so an image
///    in inline position (a substitution's copy in a paragraph, or one a
///    `:target:` wraps in a `reference`) has a location of its own: its
///    override, or its span's by rule 4, both of which a deep copy keeps.
///    [DS §8.2] has it take its parent's from 010, but 010's image branch
///    applies only to an image without a source
///    (`SP/sphinx/util/nodes.py:146`). Sphinx stamps some inline kinds
///    too, through `set_source_info` (`SP/sphinx/util/docutils.py:691-695`):
///    `pending_xref` and `download_reference` (`XRefRole`,
///    `SP/sphinx/roles.py:138-139`) and `index` (`IndexRole`,
///    `SP/sphinx/domains/index.py:115-116`). Those are mixed kinds, which
///    this rule leaves without a location (the census in [DS §6.1] has
///    102 `pending_xref`s with a source and a line and 65 with neither):
///    they need overrides before anything relies on their location;
/// 2. so has a node of a kind docutils never locates ([`UNLOCATED`]);
/// 3. so has a node docutils makes without a location and puts in a
///    parent that is not yet in the document, where `setup_child` does
///    not stamp it either (`SP/docutils/nodes.py:150-157`): a field name
///    (`SP/docutils/parsers/rst/states.py:1587-1591`, and Sphinx's doc
///    fields), a classifier (`:3018-3019`), and the title of a topic or a
///    sidebar (`SP/docutils/parsers/rst/directives/body.py:53-68`). A
///    section's title is stamped as its section is, by `setup_child`, or,
///    in a container without `document`, not at all (`states.py:495-503`);
///    as 010 gives an unstamped title its parent's location, it is counted
///    with them;
/// 4. any other node has its span's source and line, and none when the
///    line is unknown (0).
fn parse_location(tree: &TxTree, id: TxId, state: &DocumentState) -> DocLoc {
    let node = tree.node(id);
    if let Some(loc) = node.uid().and_then(|uid| state.loc_overrides.get(&uid)) {
        return *loc;
    }
    let parent = tree.parent(id).map(|parent| tree.node(parent).kind);
    if parent.is_some_and(is_text_element) && node.kind != kinds::IMAGE {
        return DocLoc::default();
    }
    match node.kind {
        kinds::TEXT | kinds::FIELD_NAME | kinds::CLASSIFIER => DocLoc::default(),
        kind if UNLOCATED.contains(&kind) => DocLoc::default(),
        kinds::TITLE if matches!(parent, Some(kinds::SECTION | "topic" | "sidebar")) => {
            DocLoc::default()
        }
        _ => span_location(node.span),
    }
}

/// The location half of Sphinx's ApplySourceWorkaround (010,
/// `apply_source_workaround`, `SP/sphinx/util/nodes.py:127-216`) for the
/// node at `id`, whose parse location is `loc`. 010 visits the tree in
/// pre-order, so each ancestor it reads stands as 010 has left it
/// ([`own_location`]). Every branch fills in a node without a source:
///
/// - a classifier takes its item's source (`:143-145`); one docutils
///   makes with an empty rawsource also takes the line before its item's
///   (`:133-142`), which only the parse knows, so that one's location is
///   recorded there, source included;
/// - an image, a title or a topic takes its parent's source and line
///   (`:146-159`, `:171-178`). The Image directive stamps every image it
///   makes and the Topic directive every topic, so the branch serves the
///   titles of topics, sidebars and sections, each taking its parent's
///   location as this model gives it;
/// - a literal block takes its nearest located ancestor's source, its
///   line unchanged (`:182-184`): a `parsed-literal` in a container
///   without `document`;
/// - a rubric, a line, an image or a field name still without one takes
///   its nearest located ancestor's source, or none (`''`), and line 0
///   (`:187-216`). The branch also takes a node whose rawsource is still
///   empty, which is a `line` with no text: nothing is located through a
///   node with no children, so that one is not modelled.
fn apply_source_workaround(tree: &TxTree, id: TxId, state: &DocumentState, loc: DocLoc) -> DocLoc {
    if loc.source.is_some() {
        return loc;
    }
    let kind = tree.node(id).kind;
    let parent = tree.parent(id);
    let parent_location =
        || parent.map_or_else(DocLoc::default, |up| own_location(tree, up, state));
    let mut loc = loc;
    match kind {
        kinds::CLASSIFIER => loc.source = parent_location().source,
        kinds::IMAGE | kinds::TITLE | "topic" => loc = parent_location(),
        kinds::LITERAL_BLOCK => loc.source = nearest_source(tree, parent, state),
        _ => {}
    }
    if loc.source.is_none()
        && matches!(
            kind,
            "rubric" | kinds::LINE | kinds::IMAGE | kinds::FIELD_NAME
        )
    {
        loc = DocLoc {
            source: nearest_source(tree, parent, state),
            line: Some(0),
        };
    }
    loc
}

/// Sphinx's `copy_source_info(src, dst)` (`SP/sphinx/util/nodes.py:691-694`):
/// the location the node at `id` hands a node made to stand in for it. Its
/// source and its line are found separately: the source of the nearest of
/// the node and its ancestors that has one (`get_node_source`) and the line
/// of the nearest that has a non-zero one (`get_node_line`,
/// `:357-362`), so a rubric 010 left at line 0 gives its own source and an
/// ancestor's line. With no source found, Python's `ValueError` is
/// suppressed before either is set: no location; with no line found, the
/// source alone.
pub fn copy_source_info(tree: &TxTree, id: TxId, state: &DocumentState) -> DocLoc {
    let Some(source) = nearest_source(tree, Some(id), state) else {
        return DocLoc::default();
    };
    let mut node = Some(id);
    let mut line = None;
    while let Some(current) = node {
        line = own_location(tree, current, state)
            .line
            .filter(|&line| line != 0);
        if line.is_some() {
            break;
        }
        node = tree.parent(current);
    }
    DocLoc {
        source: Some(source),
        line,
    }
}

/// Sphinx's `get_node_source` (`SP/sphinx/util/nodes.py:349-354`) for a
/// node without a source whose parent is `from`: the source of its
/// nearest ancestor that has one, as 010 has left it; `None` where Python
/// raises `ValueError`, which 010 turns into `''`.
fn nearest_source(tree: &TxTree, from: Option<TxId>, state: &DocumentState) -> Option<SourceRef> {
    let mut node = from;
    while let Some(current) = node {
        if let Some(source) = own_location(tree, current, state).source {
            return Some(source);
        }
        node = tree.parent(current);
    }
    None
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
    use crate::read::ReadSettings;
    use crate::rst::{parse_rst_full, ParseOptions, ParseOutput};
    use crate::transforms::state::SourceRef;
    use crate::transforms::tree::TxNode;
    use crate::transforms::{TransformCx, TransformId, TransformKey};

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

    /// Substitutions (220) over the parse of `index.rst`: each defined
    /// reference replaced by a deep copy of its definition's children.
    fn substitute(tree: &mut TxTree, state: &mut DocumentState) {
        let settings = ReadSettings::sphinx_defaults();
        TransformId::Substitutions
            .transform()
            .apply(&mut TransformCx {
                tree,
                state,
                settings: &settings,
                key: TransformKey {
                    priority: 220,
                    serial: 4,
                },
                docname: "index",
                srcdir: std::path::Path::new("<SRCDIR>"),
            });
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

    /// The locations of the nodes `pred` accepts in `rst`, parsed as
    /// Sphinx parses `index.rst`, once Substitutions (220) has copied the
    /// definitions in: in the tree's pre-order, as Sphinx prints them.
    fn locate(rst: &str, pred: impl Fn(&TxNode) -> bool) -> Vec<String> {
        let ParseOutput {
            doctree, mut state, ..
        } = parse_rst_full(
            rst,
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
        tree.findall(tree.root(), pred)
            .into_iter()
            .map(|node| render(get_source_line(&tree, node, &state), &sources))
            .collect()
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
    /// an image's `:target:` reference has the source only, a
    /// parsed-literal block its first content line, and a rubric the line
    /// its container's `setup_child` or 010 gives it.
    ///
    /// An image has its own line wherever it is, and Sphinx's
    /// ImageCollector locates `image file not readable` there
    /// (`location=node`, `SP/sphinx/environment/collectors/asset.py:96-103`):
    /// the strict aux-gate cases `docutils-0438` and `docutils-0685` expect
    /// `<SRCDIR>/index.rst:1` for an image a `:target:` wraps, while its
    /// reference has the source only. A substituted image, plain or
    /// wrapped, is a deep copy of the definition's image and is located at
    /// the definition's line, as that image is; the substituted reference
    /// has no location, so it takes the paragraph's.
    //
    // oracle (Sphinx 9.1.0, docutils 0.22.4): each snippet built as
    //   `index.rst` with the HTML oracle's `conf.py` and a `doctree-read`
    //   handler printing `utils.get_source_line(node)` for every image and
    //   reference of `doctree.findall()`; the `-w` stream prints each image
    //   at its line (the substituted ones twice, the copy and the
    //   definition's).
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
            let ours = locate(case["rst"].as_str().expect("rst"), |node| {
                node.kind == kinds::REFERENCE && attr(node, "refname") == Some("bad")
            });
            if ours != expected {
                mismatches.push(format!("{name}: {ours:?}, docutils {expected:?}"));
            }
            rows += 1;
        }
        assert!(rows >= 36, "only {rows} location cases");
        // Every image and reference, in pre-order.
        let images: [(&str, &str, &[&str]); 3] = [
            (
                "docutils-0438",
                ".. image:: pic.png\n   :target: https://example.com/page\n",
                &["<SRCDIR>/index.rst:", "<SRCDIR>/index.rst:1"],
            ),
            (
                "docutils-0685",
                ".. image:: p.png\n   :target: http://x^_y\n",
                &["<SRCDIR>/index.rst:", "<SRCDIR>/index.rst:1"],
            ),
            (
                "substituted images",
                "Title\n=====\n\nPara |img| text and |img2|.\n\n\
                 .. |img| image:: pic.png\n\n\
                 .. |img2| image:: pic2.png\n   :target: http://x.org\n\n\
                 .. image:: pic3.png\n   :target: http://y.org\n",
                &[
                    // The copies in the paragraph: `|img|`'s image, then
                    // `|img2|`'s reference and image.
                    "<SRCDIR>/index.rst:6",
                    "<SRCDIR>/index.rst:4",
                    "<SRCDIR>/index.rst:8",
                    // The definitions' own, in the same order.
                    "<SRCDIR>/index.rst:6",
                    "<SRCDIR>/index.rst:8",
                    "<SRCDIR>/index.rst:8",
                    // The last image's reference, then the image.
                    "<SRCDIR>/index.rst:",
                    "<SRCDIR>/index.rst:11",
                ],
            ),
        ];
        for (name, rst, expected) in images {
            let ours = locate(rst, |node| {
                node.kind == kinds::IMAGE || node.kind == kinds::REFERENCE
            });
            if ours != expected {
                mismatches.push(format!("{name}: {ours:?}, docutils {expected:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }

    /// Where Sphinx prints `Unknown target name` for a reference in each
    /// kind of node ApplySourceWorkaround (010) locates
    /// ([`apply_source_workaround`]), and in the lines a `line-block`
    /// directive stamps: the walk-up from every unresolved reference, in
    /// pre-order, once Substitutions (220) has copied the definitions in,
    /// as `:line` after the document's path (`:` for the path alone). A
    /// field name stands at line 0 of its field's source; a classifier at
    /// its item's source, and, made with an empty rawsource (markup right
    /// after ` : `), at the line before its item's; a directive's line `i`
    /// at the directive's line plus `i`; the title of a topic or sidebar
    /// at its parent's location. Rows (i)-(iv) are the controller's probes.
    //
    // oracle (Sphinx 9.1.0, docutils 0.22.4): each snippet built as
    //   `index.rst` with the HTML oracle's `conf.py`, its `-w` stream; a
    //   substituted reference reported in the paragraph and again in the
    //   definition (the copy Substitutions, 220, makes).
    #[test]
    fn workaround_locations_match_sphinx() {
        // A one-cell grid table holding `content`'s lines.
        let cell = |content: &str| {
            format!(
                "+------------------------+\n{}+------------------------+\n",
                content
                    .split('\n')
                    .map(|line| format!("| {line:<22} |\n"))
                    .collect::<String>()
            )
        };
        let rows: &[(&str, String, &[&str])] = &[
            (
                "(i) field name",
                "Title\n=====\n\nPara.\n\n:See fieldref_: body\n".into(),
                &[":0"],
            ),
            (
                "(ii) line-block directive",
                "Title\n=====\n\n.. line-block::\n\n   First bad1_ line\n   Second bad2_ line\n"
                    .into(),
                &[":4", ":5"],
            ),
            (
                "(iii) line block",
                "Title\n=====\n\n| First bad3_ line\n| Second bad4_ line\n".into(),
                &[":4", ":5"],
            ),
            (
                "(iv) substituted image target",
                "Title\n=====\n\nPara |i| x.\n\n.. |i| image:: p.png\n   :target: nope_\n".into(),
                &[":4", ":6"],
            ),
            // Field names: their field's source and line 0, wherever the
            // field list stands; a field body is located as before.
            ("field at the start", ":Field bad1_: body\n".into(), &[":0"]),
            (
                "fields after a paragraph",
                "Para.\n\n:Field bad1_: body\n:Other: x bad2_\n".into(),
                &[":0", ":4"],
            ),
            (
                "fields over several lines",
                "Para.\n\n:Field bad1_: body\n   more\n:B bad2_: y\n".into(),
                &[":0", ":0"],
            ),
            (
                "field in a list item",
                "- item\n\n  :Field bad1_: body\n".into(),
                &[":0"],
            ),
            (
                "field in a note",
                ".. note::\n\n   :Field bad1_: body\n".into(),
                &[":0"],
            ),
            (
                "field in only",
                ".. only:: html\n\n   :Field bad1_: body\n".into(),
                &[":0"],
            ),
            ("field in a table cell", cell(":Field bad1_: body"), &[":0"]),
            (
                "unknown doc field",
                ".. py:function:: f(x)\n\n   :foo bad2_: body\n".into(),
                &[":0"],
            ),
            // `line-block` directive lines: the directive's line plus the
            // line's index in the content, blank lines and nesting counted.
            (
                "line-block with an option",
                ".. line-block::\n   :class: x\n\n   First bad1_\n   Second bad2_\n".into(),
                &[":1", ":2"],
            ),
            (
                "line-block with blank lines",
                ".. line-block::\n   :class: x\n\n   a bad1_\n\n\n   b bad2_\n".into(),
                &[":1", ":4"],
            ),
            (
                "nested line-block",
                ".. line-block::\n\n   a bad1_\n      b bad2_\n   c bad3_\n".into(),
                &[":1", ":2", ":3"],
            ),
            (
                "line-block in a list item",
                "- item\n\n  .. line-block::\n\n     a bad1_\n     b bad2_\n".into(),
                &[":3", ":4"],
            ),
            (
                "line-block in a note",
                ".. note::\n\n   .. line-block::\n\n      a bad1_\n      b bad2_\n".into(),
                &[":3", ":4"],
            ),
            (
                "line-block in a table cell",
                cell(".. line-block::\n\n   a bad1_\n   b bad2_"),
                &[":2", ":3"],
            ),
            (
                "line block with a continuation",
                "| a\n  cont bad1_\n| b bad2_\n".into(),
                &[":1", ":3"],
            ),
            (
                "nested line block",
                "| a bad1_\n|    b bad2_\n| c bad3_\n".into(),
                &[":1", ":2", ":3"],
            ),
            // Classifiers: the item's source; with an empty rawsource also
            // the line before the item's, line 0 for an item on line 1.
            ("classifier", "term : cls bad1_\n   Def.\n".into(), &[":"]),
            (
                "classifier opening with markup",
                "Para.\n\nterm : *cls* bad1_\n   Def.\n".into(),
                &[":2"],
            ),
            (
                "classifier opening with a reference",
                "Para.\n\nterm : bad1_ x\n   Def.\n".into(),
                &[":2"],
            ),
            (
                "two classifiers",
                "Para.\n\nterm : *a* bad1_ : b bad2_\n   Def.\n".into(),
                &[":2", ":"],
            ),
            (
                "classifier on line 1",
                "term : *c* bad1_\n   Def.\n".into(),
                &[":0"],
            ),
            (
                "classifier of a second item",
                "Para.\n\nt1\n   D1.\n\nt2 : *c* bad1_\n   D2.\n".into(),
                &[":5"],
            ),
            (
                "classifier in a list item",
                "- item\n\n  term : *c* bad1_\n     Def.\n".into(),
                &[":2"],
            ),
            (
                "classifier in a table cell",
                cell("term : *c* bad1_\n   Def."),
                &[":1"],
            ),
            // Titles: a topic's and a sidebar's take their parent's
            // location, a section's its section's; an admonition's and a
            // table's are stamped at the directive's line.
            (
                "topic title",
                ".. topic:: T bad1_\n\n   Body.\n".into(),
                &[":1"],
            ),
            (
                "sidebar title and subtitle",
                ".. sidebar:: S bad1_\n   :subtitle: Sub bad2_\n\n   Body.\n".into(),
                &[":1", ":1"],
            ),
            (
                "sidebar title in a section",
                "Title\n=====\n\n.. sidebar:: S bad1_\n\n   Body.\n".into(),
                &[":4"],
            ),
            (
                "section title",
                "Title bad1_\n===========\n\nPara.\n".into(),
                &[":2"],
            ),
            (
                "admonition title",
                ".. admonition:: A bad1_\n\n   Body.\n".into(),
                &[":1"],
            ),
            (
                "table title",
                ".. table:: T bad1_\n\n   ===  ===\n   a    b\n   ===  ===\n".into(),
                &[":1"],
            ),
            (
                "figure caption and legend",
                ".. figure:: p.png\n\n   Caption bad1_\n\n   Legend bad2_\n".into(),
                &[":3", ":5"],
            ),
            // Rubrics and parsed-literal blocks in more containers without
            // `document`: the nearest located ancestor's source.
            (
                "rubric in a block quote",
                "Para.\n\n   .. rubric:: R bad1_\n\n   Text.\n".into(),
                &[":0"],
            ),
            (
                "rubric in a table cell",
                cell(".. rubric:: R bad1_\n\nText."),
                &[":0"],
            ),
            (
                "parsed-literal in a block quote",
                "Para.\n\n   .. parsed-literal::\n\n      code bad1_\n".into(),
                &[":5"],
            ),
            (
                "parsed-literal ending a table cell",
                format!(
                    "{}\nAfter.\n",
                    cell("Text.\n\n.. parsed-literal::\n\n   code bad1_")
                ),
                &[":7"],
            ),
        ];
        let mut mismatches = Vec::new();
        for (name, rst, expected) in rows {
            let ours = locate(rst, |node| {
                node.kind == kinds::REFERENCE && attr(node, "refname").is_some()
            });
            let ours: Vec<&str> = ours
                .iter()
                .map(|loc| loc.strip_prefix("<SRCDIR>/index.rst").unwrap_or(loc))
                .collect();
            if ours != *expected {
                mismatches.push(format!("{name}: {ours:?}, Sphinx {expected:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
