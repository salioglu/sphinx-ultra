//! docutils' miscellaneous transforms (`SP/docutils/transforms/misc.py`)
//! and its front-matter `DocInfo` (`SP/docutils/transforms/frontmatter.py`).

use std::sync::LazyLock;

use super::location;
use super::tree::{TxId, TxNode, TxTree};
use super::{Transform, TransformCx};
use crate::doctree::ids::{fully_normalize_name, make_id};
use crate::doctree::messages::{self, ERROR, WARNING};
use crate::doctree::pending::PyValue;
use crate::doctree::{kinds, AttrValue, Node, Span};

/// 210 `ClassAttribute` (`SP/docutils/transforms/misc.py:37-62`), the
/// transform a `.. rst-class::` (or, in docutils mode, `.. class::`)
/// without content defers to a `pending` node: the pending node's
/// classes (`details['class']`) go to the first node of
/// `pending.findall(include_self=False, descend=False, siblings=True,
/// ascend=True)` that is neither `Invisible` nor a `system_message`
/// ([`passed_over`]) — its following siblings, then its parent's, and so
/// on up, never descending — and the pending node leaves its parent.
/// It runs at priority 210 after every static transform of that priority,
/// in parse order among the pending ones ([`super::schedule`]).
///
/// With no such node, the pending node is replaced by
/// `reporter.error('No suitable element following "<directive>"
/// directive', literal_block(rawsource, rawsource), line=pending.line)`:
/// the directive's name and block text from the pending node, its line
/// the node's own, which the reporter reads as docutils does after the
/// parse ([`TransformCx::system_message_at_line`]).
pub struct ClassAttribute;

impl Transform for ClassAttribute {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let Some(pending) = cx
            .pending_record()
            .and_then(|record| cx.tree.by_uid(record.node))
        else {
            return;
        };
        let (classes, directive) = class_details(cx.tree.node(pending));
        let suitable = cx.tree.next_node(
            pending,
            |node| !passed_over(node.kind),
            false,
            false,
            true,
            true,
        );
        if let Some(element) = suitable {
            cx.tree.node_mut(element).attrs.classes.extend(classes);
            if let Some(parent) = cx.tree.parent(pending) {
                cx.tree.remove(parent, pending);
            }
            return;
        }
        let rawsource = cx
            .tree
            .node(pending)
            .rawsource()
            .unwrap_or_default()
            .to_string();
        let mut block = messages::literal_block(&rawsource);
        block.set_rawsource(&rawsource);
        let line = location::own_location(cx.tree, pending, cx.state).line;
        let error = cx.system_message_at_line(
            ERROR,
            &format!("No suitable element following \"{directive}\" directive"),
            line,
            vec![block],
        );
        cx.tree.replace_self(pending, vec![error]);
    }
}

/// The `class` and `directive` details of a ClassAttribute `pending` node.
fn class_details(pending: &TxNode) -> (Vec<String>, String) {
    let mut classes = Vec::new();
    let mut directive = String::new();
    let details = pending
        .ext
        .as_deref()
        .and_then(|ext| ext.pending.as_deref())
        .map(|pending| pending.details.as_slice())
        .unwrap_or_default();
    for (key, value) in details {
        match (key.as_str(), value) {
            ("class", PyValue::StrList(values)) => classes.clone_from(values),
            ("directive", PyValue::Str(name)) => directive.clone_from(name),
            _ => {}
        }
    }
    (classes, directive)
}

/// Whether ClassAttribute passes over a node of `kind`: docutils'
/// `Invisible` classes under Sphinx 9.1.0 (`comment`,
/// `substitution_definition`, `target`, `pending`, and Sphinx's `index`,
/// enumerated from `docutils.nodes` and `sphinx.addnodes`) and
/// `system_message`.
fn passed_over(kind: &str) -> bool {
    matches!(
        kind,
        kinds::COMMENT
            | "substitution_definition"
            | kinds::TARGET
            | "pending"
            | "index"
            | kinds::SYSTEM_MESSAGE
    )
}

/// Whether `kind` is one of docutils' `PreBibliographic` node classes,
/// what `document.first_child_not_matching_class(nodes.PreBibliographic)`
/// skips before it decides whether the document opens with bibliographic
/// fields (DocInfo) or with its docinfo (Sphinx's MetadataCollector,
/// [`crate::env::metadata`]). The complete set under Sphinx 9.1.0,
/// enumerated by walking `nodes.PreBibliographic`'s subclasses in
/// `docutils.nodes` and `sphinx.addnodes`: the `Invisible` ones
/// (`comment`, `substitution_definition`, `target`, `pending` and Sphinx's
/// `index`) plus `title`, `subtitle`, `meta`, `decoration`,
/// `system_message` and `raw`.
pub(crate) fn is_pre_bibliographic(kind: &str) -> bool {
    matches!(
        kind,
        kinds::COMMENT
            | "substitution_definition"
            | kinds::TARGET
            | "pending"
            | "index"
            | kinds::TITLE
            | kinds::SUBTITLE
            | "meta"
            | "decoration"
            | kinds::SYSTEM_MESSAGE
            | "raw"
    )
}

/// 340-008 `DocInfo` (`SP/docutils/transforms/frontmatter.py:266-…`;
/// `docinfo_xform` is on under Sphinx): when the document's first child
/// that is not `PreBibliographic` ([`is_pre_bibliographic`]) is a field
/// list, the list leaves the document and what [`extract_bibliographic`]
/// makes of it — a `docinfo`, then a dedication and an abstract topic —
/// goes in where the first child that is not `Titular` (a title, a
/// subtitle, a rubric), a decoration or a meta stood: ahead of a comment
/// or a target that preceded the list. Sphinx's MetadataCollector (880)
/// pops the docinfo again ([`crate::env::metadata::process_doc`]), so only
/// the topics stay in the persisted tree; the docinfo's content shows in
/// the metadata, in the records raised about it and in the ids of what it
/// holds.
///
/// docutils' language is Sphinx's `language`, `en` by default: only `en`'s
/// field names, labels and author separators are ported, and another
/// language reads as `en` (a documented divergence, T19a).
///
/// Sphinx's ApplySourceWorkaround (010) ran before DocInfo moved the
/// field bodies' content, so the field list's locations are frozen as 010
/// left them first ([`location::freeze_workaround`]); the nodes DocInfo
/// makes (the docinfo, its elements, the topics and their titles) have no
/// location, as docutils' current line is unset after the parse.
pub struct DocInfo;

impl Transform for DocInfo {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let root = cx.tree.root();
        let children = cx.tree.node(root).children.clone();
        let kind = |tree: &TxTree, at: usize| tree.node(children[at]).kind;
        let Some(index) = (0..children.len()).find(|&at| !is_pre_bibliographic(kind(cx.tree, at)))
        else {
            return;
        };
        if kind(cx.tree, index) != kinds::FIELD_LIST {
            return;
        }
        let biblioindex = (0..children.len())
            .find(|&at| {
                !matches!(
                    kind(cx.tree, at),
                    kinds::TITLE | kinds::SUBTITLE | "rubric" | "decoration" | "meta"
                )
            })
            .unwrap_or(index);
        let field_list = children[index];
        location::freeze_workaround(cx.tree, field_list, cx.state);
        let nodelist = extract_bibliographic(cx, field_list);
        cx.tree.remove(root, field_list);
        for (offset, node) in nodelist.into_iter().enumerate() {
            cx.tree.insert(root, biblioindex + offset, node);
        }
    }
}

/// docutils' `en` bibliographic field names (all canonical already) and
/// the label each one's topic title takes (`SP/docutils/languages/en.py`).
const BIBLIOGRAPHIC_FIELDS: [(&str, &str); 12] = [
    ("author", "Author"),
    ("authors", "Authors"),
    ("organization", "Organization"),
    ("address", "Address"),
    ("contact", "Contact"),
    ("version", "Version"),
    ("revision", "Revision"),
    ("status", "Status"),
    ("date", "Date"),
    ("copyright", "Copyright"),
    ("dedication", "Dedication"),
    ("abstract", "Abstract"),
];

/// `en`'s `author_separators`, tried in order.
const AUTHOR_SEPARATORS: [char; 2] = [';', ','];

/// docutils' `TransformError` inside `extract_bibliographic`: the field
/// stays a `field` of the docinfo.
struct KeepField;

/// `DocInfo.extract_bibliographic` (`frontmatter.py:376-427`): each field
/// of `field_list`, in order, becomes an element of the `docinfo` it
/// makes, or one of its two topics, or stays a `field` of the docinfo,
/// with its name's `make_id` as a class, when it is no bibliographic field
/// or DocInfo cannot read it ([`KeepField`], with a warning about it in
/// the field's body). The nodes for the document: the docinfo, when it
/// holds anything, then the dedication and the abstract.
fn extract_bibliographic(cx: &mut TransformCx<'_>, field_list: TxId) -> Vec<TxId> {
    let docinfo = cx.tree.new_node("docinfo");
    // The dedication and the abstract.
    let mut topics: [Option<TxId>; 2] = [None, None];
    for field in cx.tree.node(field_list).children.clone() {
        // `name = field[0][0].astext()`: the field name's first child.
        let name = cx
            .tree
            .node(field)
            .children
            .first()
            .and_then(|&field_name| cx.tree.node(field_name).children.first().copied())
            .map(|first| cx.tree.astext(first))
            .unwrap_or_default();
        let normed = fully_normalize_name(&name);
        let read = bibliographic_field(cx, field, &name, &normed, docinfo, &mut topics);
        if read.is_err() {
            if let Some(body) = cx.tree.node(field).children.last().copied() {
                let body_children = &cx.tree.node(body).children;
                if let [only] = body_children.as_slice() {
                    if is_paragraph(cx.tree.node(*only).kind) {
                        clean_rcs_keywords(cx.tree, *only);
                    }
                }
            }
            let class = make_id(&normed);
            if !class.is_empty() {
                cx.tree.node_mut(field).attrs.classes.push(class);
            }
            cx.tree.append(docinfo, field);
        }
    }
    let mut nodelist = Vec::new();
    if !cx.tree.node(docinfo).children.is_empty() {
        nodelist.push(docinfo);
    }
    nodelist.extend(topics.into_iter().flatten());
    nodelist
}

/// The body of `extract_bibliographic`'s `try` for `field`, named `name`
/// (`normed` once normalized): a bibliographic field of `en` with a body
/// becomes its element, appended to `docinfo`, or its topic in `topics`.
fn bibliographic_field(
    cx: &mut TransformCx<'_>,
    field: TxId,
    name: &str,
    normed: &str,
    docinfo: TxId,
    topics: &mut [Option<TxId>; 2],
) -> Result<(), KeepField> {
    let parts = cx.tree.node(field).children.clone();
    let &[_, body] = parts.as_slice() else {
        return Err(KeepField);
    };
    let Some(&(canonical, label)) = BIBLIOGRAPHIC_FIELDS
        .iter()
        .find(|(field_name, _)| *field_name == normed)
    else {
        return Err(KeepField);
    };
    if cx.tree.node(body).children.is_empty() {
        // `check_empty_biblio_field`.
        let text = format!("Cannot extract empty bibliographic field \"{name}\".");
        warn_in_body(cx, field, &text);
        return Err(KeepField);
    }
    match canonical {
        "authors" => extract_authors(cx, field, name, docinfo),
        "dedication" | "abstract" => {
            let slot = usize::from(canonical == "abstract");
            if topics[slot].is_some() {
                let text = format!("There can only be one \"{name}\" field.");
                warn_in_body(cx, field, &text);
                return Err(KeepField);
            }
            // `nodes.title(name, labels[canonical])`.
            let title = cx.tree.new_node(kinds::TITLE);
            cx.tree.node_mut(title).set_rawsource(name);
            let label_text = cx.tree.new_subtree(Node::text_node(label, Span::ZERO));
            cx.tree.append(title, label_text);
            let topic = cx.tree.new_node("topic");
            cx.tree
                .node_mut(topic)
                .attrs
                .classes
                .push(canonical.to_string());
            cx.tree.append(topic, title);
            for child in cx.tree.node(body).children.clone() {
                cx.tree.append(topic, child);
            }
            topics[slot] = Some(topic);
            Ok(())
        }
        _ => {
            check_compound_biblio_field(cx, field, body, name)?;
            let paragraph = cx.tree.node(body).children[0];
            clean_rcs_keywords(cx.tree, paragraph);
            let element = cx.tree.new_node(canonical);
            if canonical == "address" {
                // `address` is a `FixedTextElement`.
                cx.tree
                    .node_mut(element)
                    .set("xml:space", AttrValue::Str("preserve".to_string()));
            }
            for child in cx.tree.node(paragraph).children.clone() {
                cx.tree.append(element, child);
            }
            cx.tree.append(docinfo, element);
            Ok(())
        }
    }
}

/// `field[-1] += self.document.reporter.warning(text, base_node=field)`.
fn warn_in_body(cx: &mut TransformCx<'_>, field: TxId, text: &str) {
    let warning = cx.report(WARNING, text, field);
    if let Some(body) = cx.tree.node(field).children.last().copied() {
        cx.tree.append(body, warning);
    }
}

/// `isinstance(node, nodes.paragraph)`, Sphinx's `compact_paragraph`
/// included.
fn is_paragraph(kind: &str) -> bool {
    matches!(kind, kinds::PARAGRAPH | kinds::COMPACT_PARAGRAPH)
}

/// `DocInfo.check_compound_biblio_field` (`frontmatter.py:437-477`): a
/// field body holding one paragraph passes. A body that opens with an
/// enumerated list and whose text is one line passes too once parsed again
/// after a backslash ([`reparse_initial`]), into one paragraph: docutils
/// read an author's leading initial (`J. Smith`) as an enumerator. Any
/// other body warns, naming what it holds but its messages.
fn check_compound_biblio_field(
    cx: &mut TransformCx<'_>,
    field: TxId,
    body: TxId,
    name: &str,
) -> Result<(), KeepField> {
    let children = cx.tree.node(body).children.clone();
    if let [only] = children.as_slice() {
        if is_paragraph(cx.tree.node(*only).kind) {
            return Ok(());
        }
    }
    // The parse records the body's text for exactly this case
    // (`Body.field`'s `field_body` rawsource).
    let rawsource = children
        .first()
        .filter(|&&first| cx.tree.node(first).kind == kinds::ENUMERATED_LIST)
        .and_then(|_| cx.tree.node(body).rawsource())
        .map(str::to_string);
    if let Some(rawsource) = rawsource {
        let single_line = !rawsource
            .trim_matches(crate::utils::py_isspace)
            .contains('\n');
        if single_line {
            if let Some(paragraph) = reparse_initial(&rawsource) {
                let paragraph = cx.tree.new_subtree(paragraph);
                // `f_body.children = _document.children`: the paragraph
                // keeps the throwaway document as its parent.
                cx.tree.node_mut(body).children = vec![paragraph];
                return Ok(());
            }
        }
    }
    let content: Vec<String> = children
        .iter()
        .map(|&child| cx.tree.node(child).kind)
        .filter(|&kind| kind != kinds::SYSTEM_MESSAGE)
        .map(|kind| format!("<{kind}>"))
        .collect();
    // A body holding messages alone crashes Sphinx 9.1.0 here
    // (`content[0]` of an empty list): not modelled (decision 13), it
    // reads as an empty list.
    let content = match content.as_slice() {
        [one] => format!("a {one}"),
        many => format!("[{}]", many.join(", ")),
    };
    let text = format!(
        "Bibliographic field \"{name}\"\nmust contain a single <paragraph>, not {content}."
    );
    warn_in_body(cx, field, &text);
    Err(KeepField)
}

/// `parser.parse('\\' + f_body.rawsource, _document)` into a throwaway
/// document (`frontmatter.py:448-452`): the paragraph it makes when it
/// makes nothing else. The throwaway document's reporter writes to
/// docutils' own stream, not Sphinx's warning stream, and its
/// registrations are its own, so the parse here is a docutils-mode one
/// whose messages and identities go nowhere.
fn reparse_initial(rawsource: &str) -> Option<Node> {
    let parsed = crate::rst::parse_rst(
        &format!("\\{rawsource}"),
        &crate::rst::ParseOptions {
            source_path: "*DocInfo transform*".to_string(),
            ..crate::rst::ParseOptions::default()
        },
    );
    let mut children = parsed.root.children;
    if children.len() != 1 || children[0].kind != kinds::PARAGRAPH {
        return None;
    }
    let mut paragraph = children.pop()?;
    forget_identities(&mut paragraph);
    Some(paragraph)
}

/// Clears the identities a throwaway parse minted in `node`'s subtree,
/// which name nothing in this document.
fn forget_identities(node: &mut Node) {
    if let Some(ext) = node.ext.as_deref_mut() {
        ext.uid = None;
    }
    for child in &mut node.children {
        forget_identities(child);
    }
}

/// `DocInfo.rcs_keyword_substitutions` (`frontmatter.py:467-477`), in
/// order: Python's `\d` is Unicode's, as the regex crate's is.
static RCS_KEYWORD_SUBSTITUTIONS: LazyLock<[(regex::Regex, &str); 3]> = LazyLock::new(|| {
    let pattern = |pattern| regex::Regex::new(pattern).expect("an RCS keyword pattern compiles");
    [
        (
            pattern(r"(?i)\$Date: (\d\d\d\d)[-/](\d\d)[-/](\d\d)[ T][\d:]+[^$]* \$"),
            "${1}-${2}-${3}",
        ),
        (pattern(r"(?i)\$RCSfile: (.+),v \$"), "${1}"),
        (pattern(r"\$[a-zA-Z]+: (.+) \$"), "${1}"),
    ]
});

/// `utils.clean_rcs_keywords(paragraph, rcs_keyword_substitutions)`
/// (`SP/docutils/utils/__init__.py:497-507`, `frontmatter.py:467-477`): a
/// paragraph holding one Text has the first of the three RCS keyword
/// patterns that finds a match in its data substituted throughout, an
/// expanded `$Date: …$` reduced to its ISO date, `$RCSfile: name,v $` to
/// the name and any other `$Keyword: text $` to the text.
fn clean_rcs_keywords(tree: &mut TxTree, paragraph: TxId) {
    let children = &tree.node(paragraph).children;
    let [text] = children.as_slice() else {
        return;
    };
    let text = *text;
    if tree.node(text).kind != kinds::TEXT {
        return;
    }
    let data = text_data(tree.node(text)).to_string();
    for (pattern, substitution) in RCS_KEYWORD_SUBSTITUTIONS.iter() {
        if pattern.is_match(&data) {
            let cleaned = pattern.replace_all(&data, *substitution).into_owned();
            let node = Node::text_node_escaped(&cleaned, Span::ZERO);
            let cleaned = tree.new_subtree(node);
            tree.replace_self(text, vec![cleaned]);
            return;
        }
    }
}

/// docutils `str(Text)`: the null-escaped data when the parse kept it
/// ([`crate::doctree::NodeExt::raw`]), else the text.
fn text_data(node: &TxNode) -> &str {
    node.ext
        .as_deref()
        .and_then(|ext| ext.raw.as_deref())
        .or(node.text.as_deref())
        .unwrap_or_default()
}

/// `DocInfo.extract_authors` (`frontmatter.py:479-508`): the authors of the
/// `Authors` field, from one paragraph split at a separator
/// ([`authors_from_one_paragraph`]), from a bullet list one name an item,
/// or from paragraphs one name each, become `author` elements of an
/// `authors` element of `docinfo`. When there are none, a warning in the
/// field's body says how to write them.
fn extract_authors(
    cx: &mut TransformCx<'_>,
    field: TxId,
    name: &str,
    docinfo: TxId,
) -> Result<(), KeepField> {
    let body = cx.tree.node(field).children[1];
    let items = cx.tree.node(body).children.clone();
    let authors = match items.as_slice() {
        [only] => match cx.tree.node(*only).kind {
            kind if is_paragraph(kind) => authors_from_one_paragraph(cx, body),
            kinds::BULLET_LIST => authors_from_bullet_list(cx.tree, *only),
            _ => None,
        },
        _ => authors_from_paragraphs(cx.tree, &items),
    };
    let authors: Vec<Vec<TxId>> = authors
        .unwrap_or_default()
        .into_iter()
        .filter(|author| !author.is_empty())
        .collect();
    if authors.is_empty() {
        let separators: String = AUTHOR_SEPARATORS.iter().collect();
        let text = format!(
            "Cannot extract \"{name}\" from bibliographic field:\nBibliographic field \
             \"{name}\" must contain either\n a single paragraph (with author names separated \
             by a character from the set \"{separators}\"),\n multiple paragraphs (one per \
             author),\n or a bullet list with one author name per item.\nNote: Leading \
             initials can cause (mis)recognizing names as enumerated list."
        );
        warn_in_body(cx, field, &text);
        return Err(KeepField);
    }
    let element = cx.tree.new_node("authors");
    for author in authors {
        let node = cx.tree.new_node("author");
        for child in author {
            cx.tree.append(node, child);
        }
        cx.tree.append(element, node);
    }
    cx.tree.append(docinfo, element);
    Ok(())
}

/// `authors_from_one_paragraph` (`frontmatter.py:510-528`): the data of
/// every Text of the field's body, joined, split at the first separator
/// that splits it, but not where a backslash escaped it (`(?<!\x00);`),
/// each name stripped of whitespace and a new Text when not empty. `None`
/// for a body with no text.
fn authors_from_one_paragraph(cx: &mut TransformCx<'_>, body: TxId) -> Option<Vec<Vec<TxId>>> {
    let texts = cx.tree.findall(body, |node| node.kind == kinds::TEXT);
    let text: String = texts
        .iter()
        .map(|&text| text_data(cx.tree.node(text)))
        .collect();
    if text.is_empty() {
        return None;
    }
    let mut names = vec![text.clone()];
    for separator in AUTHOR_SEPARATORS {
        names = split_unescaped(&text, separator);
        if names.len() > 1 {
            break;
        }
    }
    let authors = names
        .iter()
        .map(|name| name.trim_matches(crate::utils::py_isspace))
        .filter(|name| !name.is_empty())
        .map(|name| {
            let node = cx
                .tree
                .new_subtree(Node::text_node_escaped(name, Span::ZERO));
            vec![node]
        })
        .collect();
    Some(authors)
}

/// `re.split('(?<!\x00)' + separator, text)`.
fn split_unescaped(text: &str, separator: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut previous = None;
    for c in text.chars() {
        if c == separator && previous != Some('\0') {
            parts.push(String::new());
        } else if let Some(part) = parts.last_mut() {
            part.push(c);
        }
        previous = Some(c);
    }
    parts
}

/// `authors_from_bullet_list` (`frontmatter.py:530-540`): each item of the
/// list but a comment must hold one paragraph, whose children are an
/// author. `None` when one does not, or there are none.
fn authors_from_bullet_list(tree: &TxTree, list: TxId) -> Option<Vec<Vec<TxId>>> {
    let mut authors = Vec::new();
    for &item in &tree.node(list).children {
        if tree.node(item).kind == kinds::COMMENT {
            continue;
        }
        match tree.node(item).children.as_slice() {
            [paragraph] if is_paragraph(tree.node(*paragraph).kind) => {
                authors.push(tree.node(*paragraph).children.clone());
            }
            _ => return None,
        }
    }
    (!authors.is_empty()).then_some(authors)
}

/// `authors_from_paragraphs` (`frontmatter.py:542-548`): a body of
/// paragraphs and comments, each paragraph's children an author. `None`
/// when the body holds anything else.
fn authors_from_paragraphs(tree: &TxTree, items: &[TxId]) -> Option<Vec<Vec<TxId>>> {
    let mut authors = Vec::new();
    for &item in items {
        match tree.node(item).kind {
            kind if is_paragraph(kind) => authors.push(tree.node(item).children.clone()),
            kinds::COMMENT => {}
            _ => return None,
        }
    }
    Some(authors)
}

/// `Transitions`' four messages (`SP/docutils/transforms/misc.py:99-107`,
/// `:135`).
const NOT_STRUCTURAL: &str = "Transition must be child of <document> or <section>.";
const BEGINS: &str = "Document or section may not begin with a transition.";
const ADJACENT: &str = "At least one body element must separate transitions; \
                        adjacent transitions are not allowed.";
const ENDS_DOCUMENT: &str = "Document may not end with a transition.";

/// 830-014 `Transitions` (`SP/docutils/transforms/misc.py:64-143`): checks
/// where each transition stands and moves one that ends a section up the
/// tree.
///
/// A transition must be a child of the document or of a section, may not
/// begin one (be its first child, or follow a title, subtitle, meta or
/// decoration) and may not follow another transition. A misplaced one is
/// reported at its own location ([`TransformCx::report`]); the message
/// joins the tree right after it only when the parent still validates with
/// a paragraph in the transition's place ([`validates_with_paragraph_at`]),
/// and the transition's index then counts the message. A transition left
/// last in a section moves behind the first ancestor that is not a last
/// child; one left last in the document stays, and the message
/// `Document may not end with a transition.` is appended to its parent.
///
/// The loop is `for node in self.document.findall(nodes.transition)`, a
/// live walk ([`TxTree::next_node`] from each transition, which this
/// transform moves but never replaces). The generator walks each child
/// list by index, so the next node it yields after a move is the moved
/// transition itself, at its new place right behind the ancestor it was
/// moved behind, and the walk here visits it there again too. Under the
/// document or a section that visit changes nothing: the transition
/// follows that ancestor, a section or a node holding one, so it neither
/// begins its parent nor follows a transition, and it is not a last child.
/// Under any other parent, an `only` or a `desc_content` holding sections,
/// it reports that the transition is not the child of a document or
/// section.
pub struct Transitions;

impl Transform for Transitions {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let is_transition = |node: &TxNode| node.kind == kinds::TRANSITION;
        let root = cx.tree.root();
        let mut next = cx
            .tree
            .next_node(root, is_transition, false, true, false, false);
        while let Some(transition) = next {
            next = if visit_transition(cx, transition) {
                // The generator's second visit, which never moves the
                // transition again: it is not a last child now.
                Some(transition)
            } else {
                cx.tree
                    .next_node(transition, is_transition, false, true, true, true)
            };
        }
    }
}

/// `Transitions.visit_transition` (`SP/docutils/transforms/misc.py:94-143`)
/// for the transition at `node`: whether it moved the transition.
fn visit_transition(cx: &mut TransformCx<'_>, node: TxId) -> bool {
    let parent = cx
        .tree
        .parent(node)
        .expect("a transition under the root has a parent");
    let mut index = child_index(cx.tree, parent, node);
    let previous = index
        .checked_sub(1)
        .map(|at| cx.tree.node(cx.tree.node(parent).children[at]).kind);
    let structural = matches!(cx.tree.node(parent).kind, kinds::DOCUMENT | kinds::SECTION);
    let message = if !structural {
        Some(NOT_STRUCTURAL)
    } else if previous
        .is_none_or(|kind| matches!(kind, kinds::TITLE | kinds::SUBTITLE | "meta" | "decoration"))
    {
        Some(BEGINS)
    } else if previous == Some(kinds::TRANSITION) {
        Some(ADJACENT)
    } else {
        None
    };
    if let Some(text) = message {
        let warning = cx.report(WARNING, text, node);
        if validates_with_paragraph_at(cx.tree, parent, index) {
            cx.tree.insert(parent, index + 1, warning);
            index += 1;
        }
    }
    if !structural {
        return false;
    }
    if index + 1 != cx.tree.node(parent).children.len() {
        // No need to move the node.
        return false;
    }
    // Climb from the transition while the node behind which it is to be
    // moved, docutils' `sibling`, is the last child of its parent,
    // `above` (`index` may count the message inserted after the
    // transition, as in docutils).
    let mut above = parent;
    while index + 1 == cx.tree.node(above).children.len() {
        let sibling = above;
        let Some(grand) = cx.tree.parent(sibling) else {
            // The transition ends the document: it stays, and the warning
            // goes behind it.
            let warning = cx.report(WARNING, ENDS_DOCUMENT, node);
            cx.tree.append(parent, warning);
            return false;
        };
        index = child_index(cx.tree, grand, sibling);
        above = grand;
    }
    cx.tree.remove(parent, node);
    cx.tree.insert(above, index + 1, node);
    true
}

/// `parent.index(child)`.
fn child_index(tree: &TxTree, parent: TxId, child: TxId) -> usize {
    tree.node(parent)
        .children
        .iter()
        .position(|&node| node == child)
        .expect("a node the walk reaches is among its parent's children")
}

/// `node.parent.validate(recursive=False)` as `visit_transition` runs it,
/// with a paragraph in place of the transition at `index`
/// (`SP/docutils/transforms/misc.py:110-119`; `Element.validate`,
/// `SP/docutils/nodes.py:1330-1356`): the element's attributes
/// ([`attributes_valid`]) and its children against its content model
/// ([`content_valid`]). Under Sphinx the document never validates:
/// `TranslationProgressTotaliser` (025) gave it `translation_progress`,
/// which docutils' `document` does not allow.
fn validates_with_paragraph_at(tree: &TxTree, parent: TxId, index: usize) -> bool {
    let element = tree.node(parent);
    let children: Vec<&str> = element
        .children
        .iter()
        .enumerate()
        .map(|(at, &child)| {
            if at == index {
                kinds::PARAGRAPH
            } else {
                tree.node(child).kind
            }
        })
        .collect();
    attributes_valid(element) && content_valid(element.kind, &children)
}

/// `Element.validate_attributes` (`SP/docutils/nodes.py:1239-1265`) for a
/// parent of a transition: every attribute it has is one its class allows,
/// the four list attributes and `source` for any element, `title` too for
/// the document, and ids and classes are valid identifiers
/// (`validate_identifier`, `:3137-3149`: `value == make_id(value)`). Names
/// and dupnames take any value, and the validators' normalizations change
/// nothing a parse made. `backrefs` belongs to the `BackLinkable` classes
/// alone, none of which holds a transition.
fn attributes_valid(element: &TxNode) -> bool {
    let attrs = &element.attrs;
    let identifiers = attrs
        .ids
        .iter()
        .chain(&attrs.classes)
        .all(|value| *value == make_id(value));
    let known = attrs
        .extra
        .iter()
        .all(|(key, _)| *key == "source" || (element.kind == kinds::DOCUMENT && *key == "title"));
    identifiers && attrs.backrefs.is_empty() && known
}

/// How many children of a content model's category may stand at its place
/// (docutils' quantifiers).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quantifier {
    /// `'.'`: exactly one.
    One,
    /// `'?'`: one at most.
    Optional,
    /// `'*'`: any number.
    Any,
}

/// The element classes a content model names (`SP/docutils/nodes.py`).
#[derive(Clone, Copy)]
enum Category {
    Title,
    Subtitle,
    Meta,
    Decoration,
    Docinfo,
    Transition,
    Section,
    Topic,
    Sidebar,
    Body,
}

impl Category {
    /// `isinstance(child, category)` for a child of `kind`.
    fn holds(self, kind: &str) -> bool {
        match self {
            Category::Title => kind == kinds::TITLE,
            Category::Subtitle => kind == kinds::SUBTITLE,
            Category::Meta => kind == "meta",
            Category::Decoration => kind == "decoration",
            Category::Docinfo => kind == "docinfo",
            Category::Transition => kind == kinds::TRANSITION,
            Category::Section => kind == kinds::SECTION,
            Category::Topic => kind == "topic",
            Category::Sidebar => kind == "sidebar",
            Category::Body => is_body(kind),
        }
    }
}

type ContentModel = &'static [(&'static [Category], Quantifier)];

/// `document.content_model` (`SP/docutils/nodes.py:1697-1705`).
const DOCUMENT_MODEL: ContentModel = &[
    (&[Category::Title], Quantifier::Optional),
    (&[Category::Subtitle], Quantifier::Optional),
    (&[Category::Meta], Quantifier::Any),
    (&[Category::Decoration], Quantifier::Optional),
    (&[Category::Docinfo], Quantifier::Optional),
    (&[Category::Transition], Quantifier::Optional),
    (
        &[
            Category::Body,
            Category::Topic,
            Category::Sidebar,
            Category::Transition,
        ],
        Quantifier::Any,
    ),
    (&[Category::Section, Category::Transition], Quantifier::Any),
];

/// `section.content_model` (`SP/docutils/nodes.py:1677-1681`).
const SECTION_MODEL: ContentModel = &[
    (&[Category::Title], Quantifier::One),
    (&[Category::Subtitle], Quantifier::Optional),
    (
        &[
            Category::Body,
            Category::Topic,
            Category::Sidebar,
            Category::Transition,
        ],
        Quantifier::Any,
    ),
    (&[Category::Section, Category::Transition], Quantifier::Any),
];

/// `Element.validate_content` (`SP/docutils/nodes.py:1267-1310`) and the
/// leftover check of `Element.validate` (`:1343-1352`) for an element of
/// `kind` whose children are of `children` kinds: the children match the
/// model's parts in order, each match passing its own position check
/// ([`position_valid`]), and none is left over. The parse puts a
/// transition under the document, a section, Sphinx's `only` or a
/// `desc_content` (the content [`crate::rst`] parses with section titles);
/// the last two keep `Element`'s empty model, so any child is left over.
fn content_valid(kind: &str, children: &[&str]) -> bool {
    let model = match kind {
        kinds::DOCUMENT => DOCUMENT_MODEL,
        kinds::SECTION => SECTION_MODEL,
        _ => &[],
    };
    let fits = |categories: &[Category], at: usize| {
        children
            .get(at)
            .is_some_and(|kind| categories.iter().any(|category| category.holds(kind)))
    };
    // The child the walk is at; `children.len()` once there is none.
    let mut at = 0;
    for &(categories, quantifier) in model {
        if !fits(categories, at) {
            if quantifier == Quantifier::One {
                return false;
            }
            continue;
        }
        if !position_valid(children, at) {
            return false;
        }
        at += 1;
        if quantifier == Quantifier::Any {
            while fits(categories, at) {
                if !position_valid(children, at) {
                    return false;
                }
                at += 1;
            }
        }
    }
    at == children.len()
}

/// `validate_position` of the child at `at`: a transition may not begin
/// its parent (come first, or after a title, subtitle, meta or
/// decoration), end it, or follow another transition
/// (`SP/docutils/nodes.py:1614-1636`); a subtitle may not come first
/// (`:1570-1575`). Any other child has no constraint.
fn position_valid(children: &[&str], at: usize) -> bool {
    match children[at] {
        kinds::TRANSITION => {
            let predecessor = at.checked_sub(1).map(|before| children[before]);
            let begins = predecessor.is_none_or(|kind| {
                matches!(kind, kinds::TITLE | kinds::SUBTITLE | "meta" | "decoration")
            });
            let ends = at + 1 == children.len();
            !(begins || ends || predecessor == Some(kinds::TRANSITION))
        }
        kinds::SUBTITLE => at != 0,
        _ => true,
    }
}

/// Whether `kind` names a docutils body element, `isinstance(node,
/// nodes.Body)`: the element classes of the `General`, `Sequential`,
/// `Admonition` and `Special` categories, `Invisible` ones included
/// (`SP/docutils/nodes.py:1398-1449` and the classes from `:2165`), and
/// Sphinx's node classes derived from them (`SP/sphinx/addnodes.py`). A
/// `glossary`, `only`, `highlightlang`, `hlist` or `centered` is not one.
fn is_body(kind: &str) -> bool {
    matches!(
        kind,
        // General.
        "paragraph"
            | "rubric"
            | "compound"
            | "container"
            | "block_quote"
            | "reference"
            | "literal_block"
            | "doctest_block"
            | "math_block"
            | "line_block"
            | "footnote"
            | "citation"
            | "image"
            | "figure"
            | "table"
            // Sequential.
            | "bullet_list"
            | "enumerated_list"
            | "definition_list"
            | "field_list"
            | "option_list"
            // Admonition.
            | "attention"
            | "caution"
            | "danger"
            | "error"
            | "hint"
            | "important"
            | "note"
            | "tip"
            | "warning"
            | "admonition"
            // Special, and Invisible.
            | "system_message"
            | "raw"
            | "comment"
            | "substitution_definition"
            | "target"
            | "pending"
            // Sphinx: General, Admonition, Invisible, and subclasses of
            // `paragraph` and `reference`.
            | "toctree"
            | "desc_content"
            | "desc"
            | "versionmodified"
            | "seealso"
            | "productionlist"
            | "index"
            | "compact_paragraph"
            | "number_reference"
            | "download_reference"
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::read::read_standalone;
    use crate::rst::ParseOptions;
    use crate::transforms::reporter::LogRecord;
    use crate::transforms::test_support::{
        differences, harness_settings, index_rst, read_snippet, SphinxRead, DOC,
    };

    /// The rows of `rows` whose read is not Sphinx's ([`differences`]),
    /// joined for an assertion message.
    fn assert_reads(rows: &[SphinxRead]) {
        let mismatches = differences(rows);
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
    }

    /// `ClassAttribute` (210-045 on, `SP/docutils/transforms/misc.py:37-62`)
    /// gives the classes of a `.. rst-class::` without content to the
    /// first node in `pending.findall(include_self=False, descend=False,
    /// siblings=True, ascend=True)` that is neither `Invisible` (a comment,
    /// a substitution definition, a target, a pending node or Sphinx's
    /// `index`) nor a `system_message`: the following siblings, then the
    /// parent's following siblings, and so on up. So the class lands on a
    /// field list itself, not on the paragraph in its body (the wave-4.5
    /// backlog bug); it climbs out of a list item or a note whose last
    /// child it is; it skips targets, an index, a comment and a
    /// substitution definition; two classes stack on one element in parse
    /// order; and one a substitution definition's directive returns runs
    /// where docutils moved the pending node, before the definition's
    /// warning. Each row is a Sphinx 9.1.0 read of the input as
    /// `index.rst` (the read-state harness, `keep_warnings = True`).
    #[test]
    fn class_attribute_climbs_and_attaches_before_field_list() {
        assert_reads(&[
            SphinxRead {
                rst: "para\n\n.. rst-class:: myclass\n\n:param x: v\n",
                body: "    <paragraph>\n        para\n    <field_list classes=\"myclass\">\n        \
                       <field>\n            <field_name>\n                param x\n            \
                       <field_body>\n                <paragraph>\n                    v\n",
                warnings: &[],
                trace: &[],
            },
            SphinxRead {
                // The doc-field transform rebuilt the field list in the
                // parse; the class reaches the list it made.
                rst: ".. py:function:: f(x)\n\n   .. rst-class:: myclass\n\n   :param x: v\n",
                body: "    <index entries=\"('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ \
                       None)\">\n    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" \
                       no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" \
                       nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" \
                       objtype=\"function\">\n        <desc_signature _toc_name=\"f()\" \
                       _toc_parts=\"('f',)\" class=\"\" classes=\"sig sig-object\" fullname=\"f\" \
                       ids=\"f\" module=\"True\">\n            <desc_name classes=\"sig-name descname\" \
                       xml:space=\"preserve\">\n                f\n            <desc_parameterlist \
                       multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" \
                       xml:space=\"preserve\">\n                <desc_parameter \
                       xml:space=\"preserve\">\n                    <desc_sig_name \
                       classes=\"n\">\n                        x\n        <desc_content>\n            \
                       <field_list classes=\"myclass\">\n                <field>\n                    \
                       <field_name>\n                        Parameters\n                    \
                       <field_body>\n                        <paragraph>\n                            \
                       <literal_strong>\n                                x\n                             \
                       -- \n                            v\n",
                warnings: &[],
                trace: &[],
            },
            SphinxRead {
                rst: "- item\n\n  .. rst-class:: up\n\nPara.\n",
                body: "    <bullet_list bullet=\"-\">\n        <list_item>\n            \
                       <paragraph>\n                item\n    <paragraph classes=\"up\">\n        \
                       Para.\n",
                warnings: &[],
                trace: &[],
            },
            SphinxRead {
                rst: ".. note::\n\n   Text.\n\n   .. rst-class:: out\n\nAfter.\n",
                body: "    <note>\n        <paragraph>\n            Text.\n    <paragraph \
                       classes=\"out\">\n        After.\n",
                warnings: &[],
                trace: &[],
            },
            SphinxRead {
                rst: ".. rst-class:: c\n\n.. _t:\n\n.. index:: x\n\n.. comment\n\n.. |s| replace:: \
                      S\n\nPara.\n",
                body: "    <index entries=\"('single',\\ 'x',\\ 'index-0',\\ '',\\ None)\" \
                       inline=\"0\">\n    <target refid=\"t\">\n    <target ids=\"index-0 t\" \
                       names=\"t\">\n    <comment xml:space=\"preserve\">\n        comment\n    \
                       <substitution_definition names=\"s\">\n        S\n    <paragraph \
                       classes=\"c\">\n        Para.\n",
                warnings: &[],
                trace: &[("parse", "target", "t", true)],
            },
            SphinxRead {
                rst: ".. rst-class:: a\n\n.. rst-class:: b\n\nPara.\n",
                body: "    <paragraph classes=\"a b\">\n        Para.\n",
                warnings: &[],
                trace: &[],
            },
            SphinxRead {
                rst: ".. |x| rst-class:: foo\n\nPara.\n",
                body: "    <system_message level=\"2\" line=\"1\" source=\"<SRCDIR>/index.rst\" \
                       type=\"WARNING\">\n        <paragraph>\n            Substitution definition \
                       \"x\" empty or invalid.\n        <literal_block xml:space=\"preserve\">\n            \
                       .. |x| rst-class:: foo\n    <paragraph classes=\"foo\">\n        Para.\n",
                warnings: &[
                    "<SRCDIR>/index.rst:1: WARNING: Substitution definition \"x\" empty or invalid.",
                    "",
                    ".. |x| rst-class:: foo [docutils]",
                ],
                trace: &[],
            },
        ]);
    }

    /// The node after a `.. rst-class::` can be a section: ClassAttribute
    /// takes the following sibling whatever it is (`state.rst_class_before_section`,
    /// Sphinx 9.1.0 read as `index.rst`), where the parse once stamped the
    /// section's first paragraph.
    #[test]
    fn rst_class_before_section_targets_the_section() {
        assert_reads(&[SphinxRead {
            rst: ".. rst-class:: special\n\nSec\n===\n\nText.\n",
            body: "    <section classes=\"special\" ids=\"sec\" names=\"sec\">\n        \
                   <title>\n            Sec\n        <paragraph>\n            Text.\n",
            warnings: &[],
            trace: &[("parse", "section", "sec", true)],
        }]);
    }

    /// With no suitable element after it, `ClassAttribute` replaces the
    /// pending node with `reporter.error('No suitable element following
    /// "<directive>" directive', literal_block(rawsource), line=pending.line)`:
    /// the directive's block text, its trailing blank lines included, and
    /// the line docutils gave the pending node, `document.current_line`
    /// when the node joined a container that has `document`: the block's
    /// last line, trailing blank lines included, or, for a node a
    /// substitution definition's directive returned, the definition's last
    /// line, where docutils moves it out. A pending node in a container
    /// without `document` (a block quote, a list item) has no line, so the
    /// reporter takes the parse's final position: line 4 past a closing
    /// block quote, and none at all after a list, where docutils keeps the
    /// `line=None` it was given, which pformat prints `True` and the
    /// message's text `None`, a text Sphinx's warning stream cannot read
    /// and logs whole as a `WARNING`. Each row is a Sphinx 9.1.0 read of
    /// the input as `index.rst` (the read-state harness); the message
    /// prints whether or not `keep_warnings` keeps it in the tree.
    #[test]
    fn class_attribute_without_target_errors() {
        assert_reads(&[
            SphinxRead {
                rst: "Para.\n\n.. rst-class:: lonely\n",
                body: "    <paragraph>\n        Para.\n    <system_message level=\"3\" line=\"3\" \
                       source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            \
                       No suitable element following \"rst-class\" directive\n        <literal_block \
                       xml:space=\"preserve\">\n            .. rst-class:: lonely\n",
                warnings: &[
                    "<SRCDIR>/index.rst:3: ERROR: No suitable element following \"rst-class\" \
                     directive",
                    "",
                    ".. rst-class:: lonely [docutils]",
                ],
                trace: &[],
            },
            SphinxRead {
                rst: "Para.\n\n.. rst-class:: a\n   b\n\n\n",
                body: "    <paragraph>\n        Para.\n    <system_message level=\"3\" line=\"6\" \
                       source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            \
                       No suitable element following \"rst-class\" directive\n        <literal_block \
                       xml:space=\"preserve\">\n            .. rst-class:: a\n               \
                       b\n            \n",
                warnings: &[
                    "<SRCDIR>/index.rst:6: ERROR: No suitable element following \"rst-class\" \
                     directive",
                    "",
                    ".. rst-class:: a",
                    "   b [docutils]",
                ],
                trace: &[],
            },
            SphinxRead {
                rst: "Para.\n\n    .. rst-class:: q\n",
                body: "    <paragraph>\n        Para.\n    <block_quote>\n        <system_message \
                       level=\"3\" line=\"4\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n            \
                       <paragraph>\n                No suitable element following \"rst-class\" \
                       directive\n            <literal_block xml:space=\"preserve\">\n                \
                       .. rst-class:: q\n",
                warnings: &[
                    "<SRCDIR>/index.rst:4: ERROR: No suitable element following \"rst-class\" \
                     directive",
                    "",
                    ".. rst-class:: q [docutils]",
                ],
                trace: &[],
            },
            SphinxRead {
                rst: "- item\n\n  .. rst-class:: x\n",
                body: "    <bullet_list bullet=\"-\">\n        <list_item>\n            \
                       <paragraph>\n                item\n            <system_message level=\"3\" \
                       line=\"True\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n                \
                       <paragraph>\n                    No suitable element following \"rst-class\" \
                       directive\n                <literal_block \
                       xml:space=\"preserve\">\n                    .. rst-class:: x\n",
                warnings: &[
                    "WARNING: <SRCDIR>/index.rst:None: (ERROR/3) No suitable element following \
                     \"rst-class\" directive",
                    "",
                    ".. rst-class:: x [docutils]",
                ],
                trace: &[],
            },
            SphinxRead {
                rst: "Para.\n\n.. |x| rst-class:: foo\n   bar\n\n",
                body: "    <paragraph>\n        Para.\n    <system_message level=\"3\" line=\"4\" \
                       source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            \
                       No suitable element following \"rst-class\" directive\n        <literal_block \
                       xml:space=\"preserve\">\n            rst-class:: foo\n               bar\n    \
                       <system_message level=\"2\" line=\"3\" source=\"<SRCDIR>/index.rst\" \
                       type=\"WARNING\">\n        <paragraph>\n            Substitution definition \
                       \"x\" empty or invalid.\n        <literal_block xml:space=\"preserve\">\n            \
                       .. |x| rst-class:: foo\n               bar\n",
                warnings: &[
                    "<SRCDIR>/index.rst:3: WARNING: Substitution definition \"x\" empty or \
                     invalid.",
                    "",
                    ".. |x| rst-class:: foo",
                    "   bar [docutils]",
                    "<SRCDIR>/index.rst:4: ERROR: No suitable element following \"rst-class\" \
                     directive",
                    "",
                    "rst-class:: foo",
                    "   bar [docutils]",
                ],
                trace: &[],
            },
        ]);

        // `keep_warnings = False`: the message prints, and 999 removes it.
        let read = read_snippet("Para.\n\n.. rst-class:: lonely\n", false);
        assert_eq!(read.body, "    <paragraph>\n        Para.\n");
        assert_eq!(
            read.warnings,
            [
                "<SRCDIR>/index.rst:3: ERROR: No suitable element following \"rst-class\" \
                 directive",
                "",
                ".. rst-class:: lonely [docutils]",
            ]
        );
    }

    /// `line=pending.line` reads the pending node's line as docutils'
    /// reporter reads a given line once the parse is over: an offset into
    /// the top-level input, included files spliced in
    /// (`get_source_and_line(lineno)`). A pending node on line 5 of an
    /// included file stands at offset 4, which is the included file's line
    /// 2 when the inclusion is the document's third line: Sphinx 9.1.0
    /// reports `inc.rst:2` for this project.
    #[test]
    fn a_class_attribute_error_reads_its_line_against_the_input() {
        let tmp = tempfile::tempdir().expect("a temporary source directory");
        std::fs::write(
            tmp.path().join("inc.rst"),
            "Para.\n\nMore.\n\n.. rst-class:: lonely\n",
        )
        .expect("inc.rst writes");
        let parse = ParseOptions {
            source_path: tmp.path().join("index.rst").display().to_string(),
            srcdir: Some(tmp.path().to_path_buf()),
            ..index_rst()
        };
        let read = read_standalone(
            "Line one.\n\n.. include:: inc.rst\n",
            &parse,
            &harness_settings(true),
        );
        let records: Vec<String> = read
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render(true))
            .collect();
        assert_eq!(records.len(), 1, "{records:#?}");
        assert!(
            records[0].ends_with(
                "inc.rst:2: ERROR: No suitable element following \"rst-class\" directive\n\n\
                 .. rst-class:: lonely [docutils]"
            ),
            "{records:#?}"
        );
        let pformat = read.outcome.doctree().root.pformat();
        assert!(
            pformat.contains("<system_message level=\"3\" line=\"2\" "),
            "{pformat}"
        );
    }

    /// `DocInfo` (340-008, `SP/docutils/transforms/frontmatter.py:266-…`)
    /// turns the document's leading field list into a `docinfo`, and
    /// Sphinx's `MetadataCollector` (880,
    /// `SP/sphinx/environment/collectors/metadata.py:35-68`) reads it into
    /// `env.metadata` and pops it: `docutils-0297`'s `:Author:` and
    /// `:Date:` become the `author` and `date` metadata and leave no field
    /// list in the persisted tree, and so does `orphan_doc/orphan`'s
    /// `:orphan:` (the env fixture's document), a field DocInfo does not
    /// know, which stays a `field` in the docinfo. Each row is a Sphinx
    /// 9.1.0 read of the input as `index.rst` (the read-state harness):
    /// the post-read doctree, the warning stream and `env.metadata`.
    #[test]
    fn docinfo_moves_and_metadata_pops() {
        /// An input, its post-read body and its metadata.
        type Row = (
            &'static str,
            &'static str,
            &'static [(&'static str, &'static str)],
        );
        let rows: [Row; 2] = [
            (
                // docutils-0297.
                ":Author: Jane\n:Date: 2026\n\nBody.\n",
                "    <paragraph>\n        Body.\n",
                &[("author", "Jane"), ("date", "2026")],
            ),
            (
                // orphan_doc/orphan.
                ":orphan:\n\nOrphan\n======\n\nThis document is not included in any toctree but \
                 is marked orphan.\n",
                "    <section ids=\"orphan\" names=\"orphan\">\n        <title>\n            \
                 Orphan\n        <paragraph>\n            This document is not included in any \
                 toctree but is marked orphan.\n",
                &[("orphan", "")],
            ),
        ];
        for (rst, body, metadata) in rows {
            let read = read_standalone(rst, &index_rst(), &harness_settings(true));
            assert_eq!(
                read.outcome.doctree().root.pformat(),
                format!("{DOC}{body}"),
                "{rst:?}"
            );
            assert!(read.diagnostics.is_empty(), "{rst:?}");
            let expected: BTreeMap<String, String> = metadata
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            assert_eq!(read.env.metadata.get("index"), Some(&expected), "{rst:?}");
        }
    }

    /// DocInfo's other shapes against Sphinx 9.1.0 (each input read as
    /// `index.rst` by the read-state harness): the dedication and the
    /// abstract become topics after the docinfo, the dedication first,
    /// inserted where the first child that is not a title, a subtitle, a
    /// rubric, a decoration or a meta stood (before a target); a second
    /// abstract, an empty bibliographic field, a field that is not one
    /// paragraph and authors DocInfo cannot split each warn at the field
    /// and stay in the docinfo; an author's leading initial, which parses
    /// as an enumerated list, is parsed again as a paragraph; and what is
    /// located through the moved content is located as Sphinx's 010 left
    /// it before DocInfo moved it: a rubric in the abstract at the field's
    /// source and line 0, a `parsed-literal` in the dedication at its own
    /// line, an author's reference, whose ancestors DocInfo made, at the
    /// parse's final position.
    #[test]
    fn docinfo_matches_docutils() {
        assert_reads(&[
            SphinxRead {
                rst: ":Abstract: a\n:Dedication: d\n:Author: x\n\nBody.\n",
                body: "    <topic classes=\"dedication\">\n        <title>\n            \
                       Dedication\n        <paragraph>\n            d\n    <topic \
                       classes=\"abstract\">\n        <title>\n            Abstract\n        \
                       <paragraph>\n            a\n    <paragraph>\n        Body.\n",
                warnings: &[],
                trace: &[],
            },
            SphinxRead {
                rst: ".. _t:\n\n:Abstract: a\n\nBody.\n",
                body: "    <topic classes=\"abstract\">\n        <title>\n            Abstract\n        \
                       <paragraph>\n            a\n    <target refid=\"t\">\n    <paragraph>\n        \
                       Body.\n",
                warnings: &[],
                trace: &[("parse", "target", "t", false)],
            },
            SphinxRead {
                rst: ":Author:\n\nBody.\n",
                body: "    <paragraph>\n        Body.\n",
                warnings: &["<SRCDIR>/index.rst:1: WARNING: Cannot extract empty bibliographic \
                             field \"Author\". [docutils]"],
                trace: &[],
            },
            SphinxRead {
                rst: ":Abstract:\n\nBody.\n",
                body: "    <paragraph>\n        Body.\n",
                warnings: &["<SRCDIR>/index.rst:1: WARNING: Cannot extract empty bibliographic \
                             field \"Abstract\". [docutils]"],
                trace: &[],
            },
            SphinxRead {
                rst: ":Author: a\n\n    b\n\nBody.\n",
                body: "    <paragraph>\n        Body.\n",
                warnings: &[
                    "<SRCDIR>/index.rst:1: WARNING: Bibliographic field \"Author\"",
                    "must contain a single <paragraph>, not [<paragraph>, <paragraph>]. \
                     [docutils]",
                ],
                trace: &[],
            },
            SphinxRead {
                rst: ":Author: *x\n\nBody.\n",
                body: "    <paragraph>\n        Body.\n",
                warnings: &[
                    "<SRCDIR>/index.rst:1: WARNING: Inline emphasis start-string without \
                     end-string. [docutils]",
                    "<SRCDIR>/index.rst:1: WARNING: Bibliographic field \"Author\"",
                    "must contain a single <paragraph>, not a <paragraph>. [docutils]",
                ],
                trace: &[
                    ("parse", "system_message", "id1", false),
                    ("parse", "problematic", "id2", false),
                ],
            },
            SphinxRead {
                rst: ":Abstract: a\n:Abstract: b\n\nBody.\n",
                body: "    <topic classes=\"abstract\">\n        <title>\n            Abstract\n        \
                       <paragraph>\n            a\n    <paragraph>\n        Body.\n",
                warnings: &["<SRCDIR>/index.rst:2: WARNING: There can only be one \"Abstract\" \
                             field. [docutils]"],
                trace: &[],
            },
            SphinxRead {
                rst: ":Authors: A. One; B. Two\n\nBody.\n",
                body: "    <paragraph>\n        Body.\n",
                warnings: &[
                    "<SRCDIR>/index.rst:1: WARNING: Cannot extract \"Authors\" from bibliographic \
                     field:",
                    "Bibliographic field \"Authors\" must contain either",
                    " a single paragraph (with author names separated by a character from the \
                     set \";,\"),",
                    " multiple paragraphs (one per author),",
                    " or a bullet list with one author name per item.",
                    "Note: Leading initials can cause (mis)recognizing names as enumerated list. \
                     [docutils]",
                ],
                trace: &[],
            },
            SphinxRead {
                rst: ":Authors: ;\n\nBody.\n",
                body: "    <paragraph>\n        Body.\n",
                warnings: &[
                    "<SRCDIR>/index.rst:1: WARNING: Cannot extract \"Authors\" from bibliographic \
                     field:",
                    "Bibliographic field \"Authors\" must contain either",
                    " a single paragraph (with author names separated by a character from the \
                     set \";,\"),",
                    " multiple paragraphs (one per author),",
                    " or a bullet list with one author name per item.",
                    "Note: Leading initials can cause (mis)recognizing names as enumerated list. \
                     [docutils]",
                ],
                trace: &[],
            },
            SphinxRead {
                rst: ":Author: J. Smith\n\nBody.\n",
                body: "    <paragraph>\n        Body.\n",
                warnings: &[],
                trace: &[],
            },
            SphinxRead {
                rst: ":Abstract: abstract bad_\n\nBody.\n",
                body: "    <topic classes=\"abstract\">\n        <title>\n            Abstract\n        \
                       <paragraph>\n            abstract \n            <problematic ids=\"id2\" \
                       refid=\"id1\">\n                bad_\n    <paragraph>\n        Body.\n",
                warnings: &["<SRCDIR>/index.rst:1: ERROR: Unknown target name: \"bad\". \
                             [docutils]"],
                trace: &[
                    ("SphinxDanglingReferences", "system_message", "id1", false),
                    ("SphinxDanglingReferences", "problematic", "id2", true),
                ],
            },
            SphinxRead {
                rst: ":Dedication: to bad_\n\nBody.\n",
                body: "    <topic classes=\"dedication\">\n        <title>\n            \
                       Dedication\n        <paragraph>\n            to \n            <problematic \
                       ids=\"id2\" refid=\"id1\">\n                bad_\n    <paragraph>\n        \
                       Body.\n",
                warnings: &["<SRCDIR>/index.rst:1: ERROR: Unknown target name: \"bad\". \
                             [docutils]"],
                trace: &[
                    ("SphinxDanglingReferences", "system_message", "id1", false),
                    ("SphinxDanglingReferences", "problematic", "id2", true),
                ],
            },
            SphinxRead {
                rst: ":Author: me bad_\n\nBody.\n",
                body: "    <paragraph>\n        Body.\n",
                warnings: &["<SRCDIR>/index.rst:4: ERROR: Unknown target name: \"bad\". \
                             [docutils]"],
                trace: &[
                    ("SphinxDanglingReferences", "system_message", "id1", false),
                    ("SphinxDanglingReferences", "problematic", "id2", false),
                ],
            },
            SphinxRead {
                rst: ":Abstract:\n\n   .. rubric:: R bad_\n\nBody.\n",
                body: "    <topic classes=\"abstract\">\n        <title>\n            Abstract\n        \
                       <rubric>\n            R \n            <problematic ids=\"id2\" \
                       refid=\"id1\">\n                bad_\n    <paragraph>\n        Body.\n",
                warnings: &["<SRCDIR>/index.rst:0: ERROR: Unknown target name: \"bad\". \
                             [docutils]"],
                trace: &[
                    ("SphinxDanglingReferences", "system_message", "id1", false),
                    ("SphinxDanglingReferences", "problematic", "id2", true),
                ],
            },
            SphinxRead {
                rst: ":Dedication:\n\n   .. parsed-literal::\n\n      lit bad_\n\nBody.\n",
                body: "    <topic classes=\"dedication\">\n        <title>\n            \
                       Dedication\n        <literal_block xml:space=\"preserve\">\n            \
                       lit \n            <problematic ids=\"id2\" refid=\"id1\">\n                \
                       bad_\n    <paragraph>\n        Body.\n",
                warnings: &["<SRCDIR>/index.rst:5: ERROR: Unknown target name: \"bad\". \
                             [docutils]"],
                trace: &[
                    ("SphinxDanglingReferences", "system_message", "id1", false),
                    ("SphinxDanglingReferences", "problematic", "id2", true),
                ],
            },
        ]);
    }

    /// `Transitions` (830-014, `SP/docutils/transforms/misc.py:64-143`):
    /// the four transition cases of [TW §6.3] (`docutils-0030` to `-0033`),
    /// then the shapes behind them. A misplaced transition is reported
    /// against its own location; the message joins the tree right after it
    /// only when its parent validates with a paragraph in its place
    /// (`validate(recursive=False)`). The document never does under Sphinx:
    /// 025 stamped `translation_progress`, which docutils' `document` does
    /// not allow. A section does when its content model holds: an `index`
    /// is a body element (`Invisible`), a `glossary` is not, and another
    /// misplaced transition fails its own position check. A transition
    /// that ends a section moves up behind the first ancestor that is not
    /// a last child; one that ends the document stays, followed by the
    /// warning. When the inserted message is the section's last child, the
    /// transition still moves (`index += 1` points at the message) and the
    /// message stays behind. One under a non-structural parent (`only`) is
    /// reported and left there, and so is one that ends a section under an
    /// `only` or in a `desc_content` and moves into it: the generator
    /// visits a moved transition again at its new place.
    ///
    /// Each row is a Sphinx 9.1.0 read of the input as `index.rst` (the
    /// read-state harness, `keep_warnings = True`): the post-read doctree
    /// and the warning stream; the four corpus cases' warnings are also
    /// their aux-gate references.
    #[test]
    fn transitions_match_docutils() {
        const BEGIN: &str = "Document or section may not begin with a transition.";
        const END: &str = "Document may not end with a transition.";
        const ADJACENT: &str = "At least one body element must separate transitions; \
                                adjacent transitions are not allowed.";
        const CHILD: &str = "Transition must be child of <document> or <section>.";
        let message = |indent: usize, line: u32, text: &str| {
            let pad = "    ".repeat(indent);
            format!(
                "{pad}<system_message level=\"2\" line=\"{line}\" source=\"<SRCDIR>/index.rst\" \
                 type=\"WARNING\">\n{pad}    <paragraph>\n{pad}        {text}\n"
            )
        };
        let record = |line: u32, text: &str| {
            format!("<SRCDIR>/index.rst:{line}: WARNING: {text} [docutils]")
        };
        let rows: Vec<(&str, String, Vec<String>)> = vec![
            (
                // docutils-0030.
                "----\n\npara\n",
                "    <transition>\n    <paragraph>\n        para\n".to_string(),
                vec![record(1, BEGIN)],
            ),
            (
                // docutils-0031.
                "para\n\n----\n",
                format!(
                    "    <paragraph>\n        para\n    <transition>\n{}",
                    message(1, 3, END)
                ),
                vec![record(3, END)],
            ),
            (
                // docutils-0032.
                "para\n\n----\n\n----\n\nend\n",
                "    <paragraph>\n        para\n    <transition>\n    <transition>\n    \
                 <paragraph>\n        end\n"
                    .to_string(),
                vec![record(5, ADJACENT)],
            ),
            (
                // docutils-0033.
                "Head\n====\n\n----\n\npara\n",
                format!(
                    "    <section ids=\"head\" names=\"head\">\n        <title>\n            \
                     Head\n        <transition>\n{}        <paragraph>\n            para\n",
                    message(2, 4, BEGIN)
                ),
                vec![record(4, BEGIN)],
            ),
            (
                "A\n=\n\n----\n\nB\n=\n\npara\n",
                format!(
                    "    <section ids=\"a\" names=\"a\">\n        <title>\n            A\n{}    \
                     <transition>\n    <section ids=\"b\" names=\"b\">\n        <title>\n            \
                     B\n        <paragraph>\n            para\n",
                    message(2, 4, BEGIN)
                ),
                vec![record(4, BEGIN)],
            ),
            (
                "A\n=\n\npara\n\n----\n",
                format!(
                    "    <section ids=\"a\" names=\"a\">\n        <title>\n            A\n        \
                     <paragraph>\n            para\n        <transition>\n{}",
                    message(2, 6, END)
                ),
                vec![record(6, END)],
            ),
            (
                "A\n=\n\n----\n\npara\n\n.. glossary::\n\n   t\n      d\n",
                "    <section ids=\"a\" names=\"a\">\n        <title>\n            A\n        \
                 <transition>\n        <paragraph>\n            para\n        \
                 <glossary sorted=\"0\">\n            <definition_list classes=\"glossary\">\n                \
                 <definition_list_item>\n                    <term ids=\"term-t\">\n                        \
                 t\n                        \
                 <index entries=\"('single',\\ 't',\\ 'term-t',\\ 'main',\\ None)\">\n                    \
                 <definition>\n                        <paragraph>\n                            d\n"
                    .to_string(),
                vec![record(4, BEGIN)],
            ),
            (
                "A\n=\n\n----\n\npara\n\n.. index:: x\n",
                format!(
                    "    <section ids=\"a\" names=\"a\">\n        <title>\n            A\n        \
                     <transition>\n{}        <paragraph>\n            para\n        \
                     <index entries=\"('single',\\ 'x',\\ 'index-0',\\ '',\\ None)\" inline=\"0\">\n        \
                     <target ids=\"index-0\">\n",
                    message(2, 4, BEGIN)
                ),
                vec![record(4, BEGIN)],
            ),
            (
                "A\n=\n\n----\n\npara\n\n----\n\n----\n\nmore\n",
                "    <section ids=\"a\" names=\"a\">\n        <title>\n            A\n        \
                 <transition>\n        <paragraph>\n            para\n        <transition>\n        \
                 <transition>\n        <paragraph>\n            more\n"
                    .to_string(),
                vec![record(4, BEGIN), record(10, ADJACENT)],
            ),
            (
                "A\n=\n\nB\n-\n\npara\n\n----\n\nC\n=\n\nz\n",
                "    <section ids=\"a\" names=\"a\">\n        <title>\n            A\n        \
                 <section ids=\"b\" names=\"b\">\n            <title>\n                B\n            \
                 <paragraph>\n                para\n    <transition>\n    \
                 <section ids=\"c\" names=\"c\">\n        <title>\n            C\n        \
                 <paragraph>\n            z\n"
                    .to_string(),
                Vec::new(),
            ),
            (
                ".. only:: html\n\n   para\n\n   ----\n\n   more\n",
                "    <only expr=\"html\">\n        <paragraph>\n            para\n        \
                 <transition>\n        <paragraph>\n            more\n"
                    .to_string(),
                vec![record(5, CHILD)],
            ),
            (
                // A transition that ends a section under `only` moves into
                // the `only`, and the generator, visiting it again there,
                // reports it.
                ".. only:: html\n\n   A\n   =\n\n   para\n\n   ----\n\n   B\n   =\n\n   text\n",
                "    <only expr=\"html\">\n        <section ids=\"a\" names=\"a\">\n            \
                 <title>\n                A\n            <paragraph>\n                para\n        \
                 <transition>\n        <section ids=\"b\" names=\"b\">\n            <title>\n                \
                 B\n            <paragraph>\n                text\n"
                    .to_string(),
                vec![record(8, CHILD)],
            ),
            (
                // The same in a `desc_content`.
                ".. py:function:: f()\n\n   A\n   =\n\n   para\n\n   ----\n\n   B\n   =\n\n   text\n",
                "    <index entries=\"('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ None)\">\n    \
                 <desc classes=\"py function\" desctype=\"function\" domain=\"py\" \
                 no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" \
                 nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n        \
                 <desc_signature _toc_name=\"f()\" _toc_parts=\"('f',)\" class=\"\" \
                 classes=\"sig sig-object\" fullname=\"f\" ids=\"f\" module=\"True\">\n            \
                 <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                \
                 f\n            <desc_parameterlist xml:space=\"preserve\">\n        \
                 <desc_content>\n            <section ids=\"a\" names=\"a\">\n                \
                 <title>\n                    A\n                <paragraph>\n                    \
                 para\n            <transition>\n            <section ids=\"b\" names=\"b\">\n                \
                 <title>\n                    B\n                <paragraph>\n                    \
                 text\n"
                    .to_string(),
                vec![record(8, CHILD)],
            ),
        ];
        let settings = harness_settings(true);
        let parse = index_rst();
        let mut mismatches = Vec::new();
        for (rst, body, warnings) in rows {
            let read = read_standalone(rst, &parse, &settings);
            let tree = read.outcome.doctree().root.pformat();
            let stream: Vec<String> = read
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.render(true))
                .collect();
            if tree != format!("{DOC}{body}") || stream != warnings {
                mismatches.push(format!(
                    "{rst:?}\n--- sphinx 9.1.0 ---\n{DOC}{body}{warnings:#?}\n--- ours ---\n\
                     {tree}{stream:#?}"
                ));
            }
            // Each message that prints is the transform's record: keyed by
            // 830-014, after the parse's records. (The INFOs
            // `SphinxDanglingReferences`, 850-039, raises about targets
            // nothing references never print.)
            for record in &read.outcome.state().log {
                if let LogRecord::Reporter { key, level, .. } = record {
                    if *level >= crate::doctree::messages::WARNING {
                        assert_eq!((key.priority, key.serial), (830, 14), "{rst:?}: {record:?}");
                    }
                }
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
    }
}
