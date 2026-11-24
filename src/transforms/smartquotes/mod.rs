//! Sphinx's smart quotes: `SphinxSmartQuotes`, the read transform at
//! 750-029 (`SP/sphinx/transforms/__init__.py:361-415`), which curls the
//! quotes and educates the dashes and ellipses of every block of text.
//!
//! [`educate`] ports the docutils functions the transform hands each
//! block's text to (`docutils.utils.smartquotes`); [`tables`] holds the
//! data they read, which `tools/gen_smartquotes_fixture.py --tables`
//! generates from docutils and Python. [`SphinxSmartQuotes`] is the
//! transform's walk over the blocks, [`settings_for`] its availability
//! test, and [`clear_raw`] drops the escape data the walk reads once the
//! read transforms are done.

pub mod educate;
pub mod tables;

use std::collections::HashSet;

use crate::config::BuildConfig;
use crate::doctree::kinds::{self, is_fixed_text_element, is_non_smartquotable, is_special};
use crate::doctree::messages::WARNING;
use crate::doctree::{is_text_element, unescape, AttrValue, Node};
use crate::transforms::tree::{TxId, TxNode, TxTree};
use crate::transforms::{Transform, TransformCx};
use educate::{
    educate_tokens, has_quotes, insert_escape_backslashes, normalize_language_tag, Actions,
    TokenKind,
};

/// What the transform does to a document, once Sphinx's availability test
/// has passed ([`settings_for`]): the educations `smartquotes_action`
/// asks for. A read without them
/// ([`crate::read::ReadSettings::smartquotes`] `None`) educates nothing.
/// The document's language, docutils' `settings.language_code`, which a
/// block no `language-*` class on it or an ancestor names is educated in,
/// is the read's own ([`crate::read::ReadSettings::language`]), the one
/// [`settings_for`] tested.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmartQuotesSettings {
    /// `smartquotes_action`, read as docutils' `educate_tokens` reads its
    /// attribute string ([`Actions::parse`]).
    pub actions: Actions,
}

impl Default for SmartQuotesSettings {
    /// Sphinx's default: `smartquotes_action = 'qDe'`
    /// (`SP/sphinx/config.py:290`).
    fn default() -> Self {
        SmartQuotesSettings {
            actions: Actions::parse("qDe"),
        }
    }
}

/// `SphinxSmartQuotes.is_available` (`SP/sphinx/transforms/__init__.py:382-401`)
/// for a build configured by `config`, whose language is `language` (the
/// configuration's, `None` read as `'en'`), by the builder named
/// `builder_name`: the settings the transform runs with, or `None` when it
/// does not run. In Sphinx's order, it does not run when `smartquotes` is
/// off, when the builder's name or the configured language is `in` the
/// `smartquotes_excludes` key that names them (a list by its names, whole:
/// `zh_CN` does not exclude `zh-CN`; a string by searching it: `dirhtml`
/// excludes `html`; [`crate::config::ExcludedNames::contains`]), or when
/// docutils has quotes for none of the language's tags
/// (`normalize_language_tag`), all without a word. The first test, of
/// `settings.smart_quotes`, reads what only a `docutils.conf` or an
/// extension sets, neither of which a build here reads (decision 16).
pub fn settings_for(
    config: &BuildConfig,
    language: &str,
    builder_name: &str,
) -> Option<SmartQuotesSettings> {
    let excludes = &config.smartquotes_excludes;
    let available = config.smartquotes
        && !excludes.builders.contains(builder_name)
        && !excludes.languages.contains(language)
        && normalize_language_tag(language)
            .iter()
            .any(|tag| has_quotes(tag));
    available.then(|| SmartQuotesSettings {
        actions: Actions::parse(&config.smartquotes_action),
    })
}

/// 750-029 `SphinxSmartQuotes` (`SP/sphinx/transforms/__init__.py:361-415`):
/// docutils' `SmartQuotes.apply` (`SP/docutils/transforms/universal.py:280-340`)
/// with Sphinx's `get_tokens`, run when the read has
/// [`SmartQuotesSettings`].
///
/// The transform educates one block of text at a time, in document order
/// (`findall(nodes.TextElement)`), so that a quote takes its context from
/// the text before it across inline elements. A block is a
/// `TextElement` that is neither a `FixedTextElement` nor `Special` and
/// whose parent is not a `TextElement`: a paragraph, a title, a term, a
/// `versionmodified` with its paragraphs (one block, so the context
/// crosses them), a paragraph in a `system_message`, and, as a
/// `pending_xref` is not a `TextElement`, the `inline` in a cross
/// reference, which is educated again after the block around it ([SQ]
/// S5). Its tokens are every Text under it but those of an
/// `option_string`: the Text under a non-smartquotable element
/// ([`kinds::is_non_smartquotable`], or one whose `support_smartquotes` is
/// `False`), up to the document, is context only, by its text; any other
/// is educated, by its escape-marked data ([`insert_escape_backslashes`]).
/// Each Text then takes its token's output as its data, as docutils
/// replaces it with `nodes.Text(newtext)`.
///
/// The language of a block is the first `language-*` class on it or an
/// ancestor, else the document's, the read's `language`
/// ([`crate::read::ReadSettings::language`]); its quotes are those of the
/// first tag of its [`normalize_language_tag`] docutils has quotes for. A
/// language with none gets ASCII quotes, and the first block of each such
/// language a WARNING reporter record, located at the block, which
/// docutils makes and puts nowhere: `No smart quotes defined for language
/// "xx".`
pub struct SphinxSmartQuotes;

impl Transform for SphinxSmartQuotes {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let read = cx.settings;
        let Some(settings) = &read.smartquotes else {
            return;
        };
        // The walk changes Text data only, so the blocks a live walk would
        // reach are the ones the tree holds now.
        let tree: &TxTree = cx.tree;
        let blocks = tree.findall(tree.root(), |node| {
            is_text_element(node.kind)
                && !is_fixed_text_element(node.kind)
                && !is_special(node.kind)
                && !node
                    .parent
                    .is_some_and(|parent| is_text_element(tree.node(parent).kind))
        });
        // The transform's `unsupported_languages`: one record per language
        // a document's blocks name.
        let mut unsupported = HashSet::new();
        for block in blocks {
            let lang = language_code(cx.tree, block, &read.language);
            let quotes = normalize_language_tag(&lang)
                .into_iter()
                .find(|tag| has_quotes(tag));
            let language = match quotes {
                Some(tag) => tag,
                None => {
                    if unsupported.insert(lang.clone()) {
                        let text = format!("No smart quotes defined for language \"{lang}\".");
                        cx.report(WARNING, &text, block);
                    }
                    String::new()
                }
            };
            educate_block(cx.tree, block, &settings.actions, &language);
        }
    }
}

/// docutils `node.get_language_code(fallback)` (`SP/docutils/nodes.py:773-786`)
/// for the node at `id`: what follows `language-` in the first class that
/// starts with it, on the node or else on its nearest ancestor that has
/// one; `fallback` when none does.
fn language_code(tree: &TxTree, id: TxId, fallback: &str) -> String {
    let mut node = Some(id);
    while let Some(current) = node {
        let classes = &tree.node(current).attrs.classes;
        if let Some(tag) = classes
            .iter()
            .find_map(|class| class.strip_prefix("language-"))
        {
            return tag.to_string();
        }
        node = tree.parent(current);
    }
    fallback.to_string()
}

/// The characters some education of [`educate_tokens`] replaces or reads:
/// a token with none of them comes out as it went in, whatever the
/// language, unless the actions stupefy, which reads curly quotes,
/// dashes and ellipses.
const EDUCABLE: [char; 7] = ['"', '\'', '-', '.', '`', '\\', '&'];

/// Educates the block at `block`: its Text as tokens ([`block_text`]),
/// through [`educate_tokens`] in `language`, each output back in the
/// place docutils puts it ([`replaced_text`], [`set_text_data`]).
fn educate_block(tree: &mut TxTree, block: TxId, actions: &Actions, language: &str) {
    let texts = block_text(tree, block);
    let tokens: Vec<(TokenKind, String)> = texts
        .iter()
        .map(|&(id, kind)| {
            let node = tree.node(id);
            let token = match kind {
                TokenKind::Plain => insert_escape_backslashes(text_data(node)),
                // `txtnode.astext()`: the unescaped text.
                TokenKind::Literal => node.text.clone().unwrap_or_default(),
            };
            (kind, token)
        })
        .collect();
    if !actions.stupefy && !tokens.iter().any(|(_, token)| token.contains(EDUCABLE)) {
        return;
    }
    let educated = educate_tokens(&tokens, actions, language);
    for ((id, _), data) in texts.into_iter().zip(educated) {
        let place = replaced_text(tree, id);
        set_text_data(tree.node_mut(place), data);
    }
}

/// The Text whose place `txtnode.parent.replace(txtnode, nodes.Text(new))`
/// puts the new Text in, for the Text at `id`
/// (`SP/docutils/transforms/universal.py:338-339`). `Element.replace`
/// finds the place with `children.index(txtnode)`
/// (`SP/docutils/nodes.py:1101-1108`), the first child that is the Text or
/// equals it, and a Text is the `str` of its data, escape markers included
/// ([`text_data`]): the first Text child of the parent whose data is the
/// Text's. That is the Text itself, unless an earlier sibling's output
/// equals the Text's source: the sibling then takes the Text's output, and
/// the Text keeps its source. A Text's own place is never one an earlier
/// output went to, which is always a place before that output's own Text.
fn replaced_text(tree: &TxTree, id: TxId) -> TxId {
    let Some(parent) = tree.parent(id) else {
        return id;
    };
    let data = text_data(tree.node(id));
    tree.node(parent)
        .children
        .iter()
        .copied()
        .find(|&child| {
            let node = tree.node(child);
            node.kind == kinds::TEXT && text_data(node) == data
        })
        .unwrap_or(id)
}

/// The Text under the block at `block`, in document order, each with the
/// token kind Sphinx's `get_tokens` gives it: literal when an element
/// between it and the document stops smart quotes (`is_smartquotable`,
/// `SP/sphinx/util/nodes.py:708-716`; [`stops_smart_quotes`]), plain
/// otherwise. The Text of an `option_string` is left out
/// (`SP/docutils/transforms/universal.py:306-308`).
fn block_text(tree: &TxTree, block: TxId) -> Vec<(TxId, TokenKind)> {
    let mut above = tree.parent(block);
    let mut literal = false;
    while let Some(ancestor) = above {
        literal |= stops_smart_quotes(tree.node(ancestor));
        above = tree.parent(ancestor);
    }
    let mut texts = Vec::new();
    collect_text(tree, block, literal, &mut texts);
    texts
}

/// [`block_text`] under the element at `id`, whose ancestors above it stop
/// smart quotes when `literal_above` says so.
fn collect_text(tree: &TxTree, id: TxId, literal_above: bool, texts: &mut Vec<(TxId, TokenKind)>) {
    let node = tree.node(id);
    let literal = literal_above || stops_smart_quotes(node);
    for &child in &node.children {
        if tree.node(child).kind != kinds::TEXT {
            collect_text(tree, child, literal, texts);
        } else if node.kind != kinds::OPTION_STRING {
            let kind = if literal {
                TokenKind::Literal
            } else {
                TokenKind::Plain
            };
            texts.push((child, kind));
        }
    }
}

/// Whether the element `node` makes the Text under it literal for
/// `is_smartquotable`: one of `NON_SMARTQUOTABLE_PARENT_NODES`
/// ([`kinds::is_non_smartquotable`]), or an element whose
/// `support_smartquotes` is `False` (pformat `0`).
fn stops_smart_quotes(node: &TxNode) -> bool {
    is_non_smartquotable(node.kind)
        || matches!(node.get("support_smartquotes"), Some(AttrValue::Int(0)))
}

/// docutils `str(Text)`: the escape-marked data the parse kept
/// ([`crate::doctree::NodeExt::raw`]), else the text, which has no marker.
fn text_data(node: &TxNode) -> &str {
    node.ext
        .as_deref()
        .and_then(|ext| ext.raw.as_deref())
        .or(node.text.as_deref())
        .unwrap_or_default()
}

/// `nodes.Text(data)` in the place of the Text `node`: its text is `data`
/// unescaped, and `data` itself is kept while it holds an escape marker,
/// for a block in a cross reference that the walk educates again.
fn set_text_data(node: &mut TxNode, data: String) {
    node.text = Some(unescape(&data, false));
    let raw = data.contains('\0').then(|| data.into_boxed_str());
    match node.ext.as_deref_mut() {
        Some(ext) => ext.raw = raw,
        None if raw.is_some() => node.ext.get_or_insert_with(Box::default).raw = raw,
        None => {}
    }
}

/// Drops the escape-marked data the parse keeps beside a Text's text
/// ([`crate::doctree::NodeExt::raw`]) from every node of the tree at
/// `node`, with any side-car that held nothing else. Stage R ends with it,
/// after SmartQuotes, the last transform that reads the data
/// ([`crate::transforms::run_read_transforms`]); nothing persists it.
pub fn clear_raw(node: &mut Node) {
    if let Some(ext) = node.ext.as_deref_mut() {
        ext.raw = None;
        if ext.uid.is_none() && ext.rawsource.is_none() && ext.pending.is_none() {
            node.ext = None;
        }
    }
    for child in &mut node.children {
        clear_raw(child);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::SmartquotesExcludes;
    use crate::doctree::{Doctree, Span};
    use crate::read::ReadSettings;
    use crate::transforms::reporter::render_records;
    use crate::transforms::state::DocumentState;
    use crate::transforms::test_support::{harness_settings, read_snippet_with};
    use crate::transforms::{TransformId, READ_TRANSFORMS};

    fn element(kind: &'static str, children: Vec<Node>) -> Node {
        let mut node = Node::elem(kind, Span::ZERO);
        node.children = children;
        node
    }

    /// `element` with the scalar attributes `attrs`.
    fn with(kind: &'static str, attrs: &[(&'static str, &str)], children: Vec<Node>) -> Node {
        let mut node = element(kind, children);
        for &(key, value) in attrs {
            node.set(key, AttrValue::Str(value.to_string()));
        }
        node
    }

    fn classed(kind: &'static str, classes: &[&str], children: Vec<Node>) -> Node {
        let mut node = element(kind, children);
        node.attrs.classes = classes.iter().map(|class| class.to_string()).collect();
        node
    }

    fn text(text: &str) -> Node {
        Node::text_node(text, Span::ZERO)
    }

    /// The 750 row's transform over the document `children` make, with
    /// Sphinx's default settings: the document's pformat and the records
    /// the transform logged, rendered.
    fn educated(children: Vec<Node>) -> (String, Vec<String>) {
        let doctree = Doctree {
            root: element(kinds::DOCUMENT, children),
            sources: vec!["<SRCDIR>/index.rst".to_string()],
        };
        let mut state = DocumentState::default();
        let mut tree = TxTree::from_parse(doctree, &mut state);
        let read = ReadSettings {
            smartquotes: Some(SmartQuotesSettings::default()),
            ..ReadSettings::sphinx_defaults()
        };
        let (key, _) = READ_TRANSFORMS
            .iter()
            .find(|(_, id)| *id == TransformId::SphinxSmartQuotes)
            .copied()
            .expect("the 750 row");
        TransformId::SphinxSmartQuotes
            .transform()
            .apply(&mut TransformCx {
                tree: &mut tree,
                state: &mut state,
                settings: &read,
                key,
                docname: "index",
                srcdir: Path::new("<SRCDIR>"),
            });
        let doctree = tree.into_doctree();
        let records = render_records(
            std::mem::take(&mut state.log),
            &doctree.sources,
            "<SRCDIR>/index.rst",
        )
        .into_iter()
        .map(|(_, record)| record.render(true))
        .collect();
        (doctree.root.pformat(), records)
    }

    /// A `versionmodified` is a `TextElement` that holds paragraphs, so it
    /// is one block and the context crosses into its second paragraph: a
    /// quote after `here` closes (`”Second”`); the paragraph after it is a
    /// block of its own.
    //
    // oracle: [SQ] S5 `probe_va.out` (Sphinx 9.1.0, smart quotes on).
    #[test]
    fn versionmodified_is_one_block_across_its_paragraphs() {
        let (pformat, records) = educated(vec![
            with(
                "versionmodified",
                &[("type", "versionadded"), ("version", "1.0")],
                vec![
                    with(
                        kinds::PARAGRAPH,
                        &[("translatable", "0")],
                        vec![
                            classed(
                                "inline",
                                &["versionmodified", "added"],
                                vec![text("Added in version 1.0: ")],
                            ),
                            text("First paragraph ends here"),
                        ],
                    ),
                    element(kinds::PARAGRAPH, vec![text("\"Second\" paragraph.")]),
                ],
            ),
            element(kinds::PARAGRAPH, vec![text("\"After\".")]),
        ]);
        assert_eq!(
            pformat,
            "<document>\n    \
             <versionmodified type=\"versionadded\" version=\"1.0\">\n        \
             <paragraph translatable=\"0\">\n            \
             <inline classes=\"versionmodified added\">\n                \
             Added in version 1.0: \n            \
             First paragraph ends here\n        \
             <paragraph>\n            \
             \u{201d}Second\u{201d} paragraph.\n    \
             <paragraph>\n        \
             \u{201c}After\u{201d}.\n"
        );
        assert!(records.is_empty(), "{records:?}");
    }

    /// A `pending_xref` is inline but not a `TextElement`, so the `inline`
    /// in it is a block of its own, educated after the paragraph around
    /// it: the paragraph's pass takes the literal backslash a Sphinx role
    /// leaves in `a\'b` with the quote it escapes, and the inline's pass
    /// curls the quote (`a’b`). Text in the paragraph itself gets the one
    /// pass (`a'b`).
    //
    // oracle: [SQ] S5 `probe_edges.out` `role_literal_backslash` and
    //   `probe_edges2.out` `xref_in_title_double_pass`.
    #[test]
    fn a_block_in_a_pending_xref_is_educated_a_second_time() {
        let (pformat, _) = educated(vec![
            element(
                kinds::PARAGRAPH,
                vec![element(
                    kinds::PENDING_XREF,
                    vec![classed(
                        "inline",
                        &["xref", "std", "std-ref"],
                        vec![text("a\\'b")],
                    )],
                )],
            ),
            element(kinds::PARAGRAPH, vec![text("a\\'b")]),
        ]);
        assert_eq!(
            pformat,
            "<document>\n    \
             <paragraph>\n        \
             <pending_xref>\n            \
             <inline classes=\"xref std std-ref\">\n                \
             a\u{2019}b\n    \
             <paragraph>\n        \
             a'b\n"
        );
    }

    /// The Text of an `option_string` is no token at all: `--long` keeps
    /// both hyphens, while the option's argument and its description are
    /// educated.
    //
    // oracle: [SQ] S6 `probe_config.out` (option lists).
    #[test]
    fn option_strings_are_left_out() {
        let (pformat, _) = educated(vec![element(
            kinds::OPTION_LIST,
            vec![element(
                kinds::OPTION_LIST_ITEM,
                vec![
                    element(
                        kinds::OPTION_GROUP,
                        vec![element(
                            kinds::OPTION,
                            vec![
                                element(kinds::OPTION_STRING, vec![text("--long")]),
                                with(
                                    kinds::OPTION_ARGUMENT,
                                    &[("delimiter", "=")],
                                    vec![text("<\"q\">")],
                                ),
                            ],
                        )],
                    ),
                    element(
                        kinds::DESCRIPTION,
                        vec![element(kinds::PARAGRAPH, vec![text("Desc it's.")])],
                    ),
                ],
            )],
        )]);
        assert_eq!(
            pformat,
            "<document>\n    \
             <option_list>\n        \
             <option_list_item>\n            \
             <option_group>\n                \
             <option>\n                    \
             <option_string>\n                        \
             --long\n                    \
             <option_argument delimiter=\"=\">\n                        \
             <\u{201d}q\u{201d}>\n            \
             <description>\n                \
             <paragraph>\n                    \
             Desc it\u{2019}s.\n"
        );
    }

    /// A block's Text is literal when an element above the block stops
    /// smart quotes too, not only one between the Text and the block: the
    /// `inline` of a `pending_xref` in a `literal_block` (a
    /// `parsed-literal`'s `:ref:`) is a block of its own, and its quotes
    /// and dashes stay as they are; so does the literal block's own text.
    //
    // oracle: `is_smartquotable` walks from the Text's parent to the
    //   document (`SP/sphinx/util/nodes.py:708-716`); the snippet corpus'
    //   `sx_smartquotes.skipped_blocks` (Sphinx 9.1.0, smart quotes on)
    //   keeps the parsed-literal's `"T"` straight.
    #[test]
    fn a_block_under_a_literal_block_stays_literal() {
        let (pformat, _) = educated(vec![element(
            kinds::LITERAL_BLOCK,
            vec![
                text("\"parsed\" "),
                element(
                    kinds::PENDING_XREF,
                    vec![classed(
                        "inline",
                        &["xref", "std", "std-ref"],
                        vec![text("\"T\" -- x")],
                    )],
                ),
            ],
        )]);
        assert_eq!(
            pformat,
            "<document>\n    \
             <literal_block>\n        \
             \"parsed\" \n        \
             <pending_xref>\n            \
             <inline classes=\"xref std std-ref\">\n                \
             \"T\" -- x\n"
        );
    }

    /// A `pending_xref` whose `support_smartquotes` is `False` (Sphinx's
    /// citation reference) makes the Text of the `inline` in it literal for
    /// both blocks that hold it: the paragraph's, and the inline's own,
    /// whose element above stops smart quotes; the paragraph's text around
    /// it is educated.
    //
    // oracle: `is_smartquotable` (`SP/sphinx/util/nodes.py:708-716`) on
    //   `CitationReferenceTransform`'s node
    //   (`SP/sphinx/domains/citation.py:150-176`).
    #[test]
    fn a_pending_xref_without_smart_quotes_keeps_its_inline() {
        let mut xref = with(
            kinds::PENDING_XREF,
            &[("refdomain", "citation"), ("reftype", "ref")],
            vec![element("inline", vec![text("\"T\" -- x")])],
        );
        xref.set("support_smartquotes", AttrValue::Int(0));
        let (pformat, _) = educated(vec![element(
            kinds::PARAGRAPH,
            vec![text("See "), xref, text(" -- \"it\".")],
        )]);
        assert_eq!(
            pformat,
            "<document>\n    \
             <paragraph>\n        \
             See \n        \
             <pending_xref refdomain=\"citation\" reftype=\"ref\" support_smartquotes=\"0\">\n            \
             <inline>\n                \
             \"T\" -- x\n         \
             \u{2013} \u{201c}it\u{201d}.\n"
        );
    }

    /// A block no `language-*` class names is educated in the read's
    /// language ([`ReadSettings::language`]), one with such a class in its
    /// own.
    //
    // oracle: Sphinx 9.1.0 through tools/gen_sphinx_fixture.py's harness,
    //   `language = 'de'`, smart quotes on (wave 5a T16b fix round 1,
    //   scratch `t16bf/probe_language.out`): the body as asserted.
    #[test]
    fn a_block_is_educated_in_the_reads_language() {
        let settings = ReadSettings {
            language: "de".to_string(),
            smartquotes: Some(SmartQuotesSettings::default()),
            ..harness_settings(true)
        };
        assert_eq!(
            read_snippet_with(
                "\"Hi\" -- it's 'q'.\n\n.. rst-class:: language-en\n\n\"En\" 'q'.\n",
                &settings
            )
            .body,
            "    <paragraph>\n        \
             \u{201e}Hi\u{201c} \u{2013} it\u{2019}s \u{201a}q\u{2018}.\n    \
             <paragraph classes=\"language-en\">\n        \
             \u{201c}En\u{201d} \u{2018}q\u{2019}.\n"
        );
    }

    /// docutils puts each output back with `txtnode.parent.replace(txtnode,
    /// nodes.Text(newtext))`, and `Element.replace` finds the place with
    /// `children.index`, which compares a Text as the string it is, escape
    /// markers included: an earlier sibling whose output equals a later
    /// Text's source takes the later Text's output, and the later Text
    /// keeps its source. `&#34;` is no entity in reStructuredText, but
    /// docutils' `smartquotes` reads it as `"` when it restores its escapes,
    /// so `&#34;x ` comes out as `"x `, the next Text's source.
    //
    // oracle: Sphinx 9.1.0 through tools/gen_sphinx_fixture.py's harness,
    //   smart quotes on (wave 5a T16b fix round 1, scratch
    //   `t16bf/probe_replace_sphinx.out`; the review's
    //   `review16b/probe_replace.py` input first): each body as asserted.
    #[test]
    fn an_output_goes_where_element_replace_finds_its_text() {
        let settings = ReadSettings {
            smartquotes: Some(SmartQuotesSettings::default()),
            ..harness_settings(true)
        };
        let paragraph = |texts: &[&str]| {
            let mut body = String::from("    <paragraph>\n");
            for (text, emphasis) in texts.iter().zip(["y", "z", "w"]) {
                body.push_str(&format!(
                    "        {text}\n        <emphasis>\n            {emphasis}\n"
                ));
            }
            body
        };
        let rows: [(&str, &[&str]); 5] = [
            // The first Text's output is the second's source.
            ("&#34;x *y*\"x *z*\n", &["\u{201d}x ", "\"x "]),
            // Each later Text's output lands one place earlier.
            (
                "&#34;x *y*\"x *z*\"x *w*\n",
                &["\u{201d}x ", "\u{201d}x ", "\"x "],
            ),
            // No earlier sibling equals the later Text.
            ("\"x *y*\"x *z*\n", &["\u{201c}x ", "\u{201d}x "]),
            // `"\x ` is `"\0x ` to docutils: no sibling equals it.
            ("&#34;x *y*\"\\x *z*\n", &["\"x ", "\u{201d}x "]),
            // `"x\ ` is `"x\0 `, and so is the first Text's output.
            ("&#34;x\\ *y*\"x\\ *z*\n", &["\u{201d}x", "\"x"]),
        ];
        for (rst, texts) in rows {
            assert_eq!(
                read_snippet_with(rst, &settings).body,
                paragraph(texts),
                "{rst:?}"
            );
        }
    }

    /// Text under an element whose `support_smartquotes` is `False` (pformat
    /// `0`; a py default value, a citation label) is literal: it stays as it
    /// is, and its last character is the next token's context.
    //
    // oracle: `is_smartquotable` (`SP/sphinx/util/nodes.py:708-716`).
    #[test]
    fn text_under_support_smartquotes_false_is_literal() {
        let mut default_value = classed("inline", &["default_value"], vec![text("'q' -- x")]);
        default_value.set("support_smartquotes", AttrValue::Int(0));
        let (pformat, _) = educated(vec![element(
            kinds::PARAGRAPH,
            vec![text("a "), default_value, text("\" it's")],
        )]);
        assert_eq!(
            pformat,
            "<document>\n    \
             <paragraph>\n        \
             a \n        \
             <inline classes=\"default_value\" support_smartquotes=\"0\">\n            \
             'q' -- x\n        \
             \u{201d} it\u{2019}s\n"
        );
    }

    /// A `system_message` is `Special` but no `TextElement`, so its
    /// paragraph is an ordinary block, educated as docutils' own
    /// SmartQuotes would not; its literal block is left alone.
    //
    // oracle: [SQ] S6 `probe_cases.out` `system_messages` (keep_warnings).
    #[test]
    fn a_system_message_paragraph_is_educated() {
        let mut message = element(
            kinds::SYSTEM_MESSAGE,
            vec![
                element(
                    kinds::PARAGRAPH,
                    vec![text("Unknown directive type \"bogus\".")],
                ),
                element(kinds::LITERAL_BLOCK, vec![text(".. bogus:: \"x\" -- y")]),
            ],
        );
        message.set("level", AttrValue::Int(3));
        message.set("type", AttrValue::Str("ERROR".to_string()));
        let (pformat, _) = educated(vec![message]);
        assert_eq!(
            pformat,
            "<document>\n    \
             <system_message level=\"3\" type=\"ERROR\">\n        \
             <paragraph>\n            \
             Unknown directive type \u{201c}bogus\u{201d}.\n        \
             <literal_block>\n            \
             .. bogus:: \"x\" -- y\n"
        );
    }

    /// A `substitution_definition` is `Special`: its own text stays
    /// straight, but the `inline` of a `pending_xref` in it is a block of
    /// its own, and educated.
    //
    // oracle: [SQ] S5 `probe_edges2.out` `subst_def_with_xref`.
    #[test]
    fn a_substitution_definition_is_skipped_but_its_xref_is_not() {
        let mut definition = element(
            "substitution_definition",
            vec![
                element(
                    kinds::PENDING_XREF,
                    vec![classed(
                        "inline",
                        &["xref", "std", "std-ref"],
                        vec![text("\"T\" -- t")],
                    )],
                ),
                text(" and \"plain\""),
            ],
        );
        definition.attrs.names = vec!["x".to_string()];
        let (pformat, _) = educated(vec![definition]);
        assert_eq!(
            pformat,
            "<document>\n    \
             <substitution_definition names=\"x\">\n        \
             <pending_xref>\n            \
             <inline classes=\"xref std std-ref\">\n                \
             \u{201c}T\u{201d} \u{2013} t\n         \
             and \"plain\"\n"
        );
    }

    /// An inline `target` is `Special` too, but Sphinx's `is_smartquotable`
    /// has no `Special` rule, so its text is educated with the paragraph's
    /// (docutils' own SmartQuotes would leave it).
    //
    // oracle: [SQ] S6 `probe_cases.out` `references`.
    #[test]
    fn inline_target_text_is_educated() {
        let mut target = element(kinds::TARGET, vec![text("\"inline\" target")]);
        target.attrs.ids = vec!["inline-target".to_string()];
        let (pformat, _) = educated(vec![element(
            kinds::PARAGRAPH,
            vec![text("An "), target, text(" here.")],
        )]);
        assert_eq!(
            pformat,
            "<document>\n    \
             <paragraph>\n        \
             An \n        \
             <target ids=\"inline-target\">\n            \
             \u{201c}inline\u{201d} target\n         \
             here.\n"
        );
    }

    /// A block whose language docutils has no quotes for keeps ASCII quotes
    /// but still gets its dashes, ellipses and in-word apostrophes (`'80s`
    /// opens: DECADE is English only), and the language is reported once
    /// per document, located at the first such block.
    //
    // oracle: [SQ] S4 `probe_edges2.out`, S12 `probe_warnline.out`.
    #[test]
    fn an_unsupported_language_keeps_ascii_quotes_and_warns_once() {
        let located = |line, children| {
            let mut paragraph = classed(kinds::PARAGRAPH, &["language-xx"], children);
            paragraph.span = Span { line, ..Span::ZERO };
            paragraph
        };
        let (pformat, records) = educated(vec![
            located(3, vec![text("\"Unknown\" it's -- y... the '80s.")]),
            located(5, vec![text("\"Unknown\" again.")]),
        ]);
        assert_eq!(
            pformat,
            "<document>\n    \
             <paragraph classes=\"language-xx\">\n        \
             \"Unknown\" it\u{2019}s \u{2013} y\u{2026} the '80s.\n    \
             <paragraph classes=\"language-xx\">\n        \
             \"Unknown\" again.\n"
        );
        assert_eq!(
            records,
            [
                "<SRCDIR>/index.rst:3: WARNING: No smart quotes defined for language \"xx\". \
              [docutils]"
            ]
        );
    }

    /// `is_available`'s gates, in order: `smartquotes` off, the builder
    /// or the configuration's language `in` its `smartquotes_excludes` key,
    /// a language docutils has no quotes for; otherwise the action string.
    /// A list holds a name whole (`zh-CN` is not `zh_CN`, `en` not
    /// `en_GB`), a tuple too; a string, as `-D smartquotes_excludes.<key>`
    /// gives it or conf.py writes it, holds every name that occurs in it
    /// (`dirhtml` excludes `html`, `en_GB` excludes `en`), and a
    /// comma-separated one is no list (`man,text` excludes no `html`).
    //
    // oracle: [SQ] S2 `probe_conftypes.out` and `probe_config.out`; the
    //   string values: Sphinx 9.1.0 `-b html` and `-b dummy` builds (wave
    //   5a T16b fix round 1, scratch `t16bf/probe_excludes.out`), smart
    //   quotes on exactly where `Some` is asserted.
    #[test]
    fn settings_follow_is_available() {
        use crate::config::ExcludedNames;

        let config = BuildConfig::default();
        assert_eq!(
            settings_for(&config, "en", "html"),
            Some(SmartQuotesSettings::default())
        );
        assert_eq!(
            settings_for(&config, "en", "dummy"),
            Some(SmartQuotesSettings::default())
        );
        let off = BuildConfig {
            smartquotes: false,
            ..BuildConfig::default()
        };
        assert_eq!(settings_for(&off, "en", "html"), None);
        for builder in ["man", "text"] {
            assert_eq!(settings_for(&config, "en", builder), None, "{builder}");
        }
        for language in ["ja", "zh_CN", "zh_TW", "xx"] {
            assert_eq!(settings_for(&config, language, "html"), None, "{language}");
        }
        assert_eq!(
            settings_for(&config, "zh-CN", "html"),
            Some(SmartQuotesSettings::default())
        );
        let excluding_dummy = BuildConfig {
            smartquotes_excludes: SmartquotesExcludes {
                languages: ExcludedNames::default(),
                builders: ExcludedNames::Names(vec!["dummy".to_string()]),
            },
            smartquotes_action: "qe".to_string(),
            ..BuildConfig::default()
        };
        assert_eq!(settings_for(&excluding_dummy, "en", "dummy"), None);
        assert_eq!(
            settings_for(&excluding_dummy, "ja", "html"),
            Some(SmartQuotesSettings {
                actions: Actions::parse("qe"),
            })
        );

        let excluding = |languages: ExcludedNames, builders: ExcludedNames| BuildConfig {
            smartquotes_excludes: SmartquotesExcludes {
                languages,
                builders,
            },
            ..BuildConfig::default()
        };
        let text = |value: &str| ExcludedNames::Text(value.to_string());
        let names = |values: &[&str]| {
            ExcludedNames::Names(values.iter().map(|value| value.to_string()).collect())
        };
        let none = ExcludedNames::default;
        let rows = [
            (excluding(none(), text("dirhtml")), "html", false),
            (excluding(none(), text("html5")), "html", false),
            (excluding(none(), text("html")), "html", false),
            (excluding(none(), text("htm")), "html", true),
            (excluding(none(), text("man,text")), "html", true),
            (excluding(none(), text("")), "html", true),
            (excluding(none(), names(&["html"])), "html", false),
            (excluding(none(), text("dummyx")), "dummy", false),
            (excluding(none(), text("html")), "dummy", true),
            (excluding(text("en_GB"), none()), "html", false),
            (excluding(text("e"), none()), "html", true),
            (excluding(text(""), none()), "html", true),
            (excluding(names(&["en_GB"]), none()), "html", true),
        ];
        for (config, builder, educated) in rows {
            assert_eq!(
                settings_for(&config, "en", builder).is_some(),
                educated,
                "{:?} for {builder}",
                config.smartquotes_excludes
            );
        }
    }

    /// The std domain notes a glossary term while the glossary directive
    /// runs (`_note_term`, `SP/sphinx/domains/std/__init__.py:375-407`),
    /// before SmartQuotes, from the parse's record of it (T5c, T13a): the
    /// term in the tree is educated, its key among the domain's `objects`
    /// and `terms` stays straight.
    //
    // oracle: [SQ] S11 `probe_registrations.out` (Sphinx 9.1.0):
    //   `std terms: [('term', '"Term" -- x')]` and the educated tree; the
    //   env fixture's `smartquotes_resolved` project pins both too.
    #[test]
    fn an_educated_glossary_term_keeps_its_straight_std_key() {
        let project = crate::transforms::test_support::merge_project(&[(
            "index",
            ".. glossary::\n\n   \"Term\" -- x\n      Def \"x\".\n",
        )]);
        let (_, doctree) = &project.doctrees[0];
        let pformat = doctree.root.pformat();
        assert!(
            pformat.contains(
                "<term ids=\"term-Term-x\">\n                    \
                 \u{201c}Term\u{201d} \u{2013} x\n"
            ),
            "{pformat}"
        );
        assert!(pformat.contains("Def \u{201c}x\u{201d}.\n"), "{pformat}");
        let entry = ("index".to_string(), "term-Term-x".to_string());
        assert_eq!(
            project
                .env
                .std
                .objects
                .get(&("term".to_string(), "\"Term\" -- x".to_string())),
            Some(&entry)
        );
        assert_eq!(project.env.std.terms.get("\"term\" -- x"), Some(&entry));
    }

    /// The read's escape data is dropped wherever it is, and a side-car
    /// that held nothing else goes with it; the rest of a side-car stays.
    #[test]
    fn clear_raw_drops_the_escape_data() {
        let mut target = element(kinds::TARGET, vec![]);
        target.set_rawsource("_`x`");
        let mut root = element(
            kinds::DOCUMENT,
            vec![element(
                kinds::PARAGRAPH,
                vec![Node::text_node_escaped("a\u{0} b", Span::ZERO), target],
            )],
        );
        clear_raw(&mut root);
        let paragraph = &root.children[0];
        assert!(paragraph.children[0].ext.is_none(), "the side-car goes");
        assert_eq!(paragraph.children[0].text_data(), "ab");
        assert_eq!(paragraph.children[1].rawsource(), Some("_`x`"));
    }
}
