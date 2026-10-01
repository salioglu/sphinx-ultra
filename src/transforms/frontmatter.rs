//! The document's front matter: docutils' DocInfo transform
//! (`docutils/transforms/frontmatter.py:266-548`, priority 340), which turns
//! the document's leading field list into a `docinfo` node and the
//! dedication/abstract topics, and the half of Sphinx's MetadataCollector
//! (`sphinx/environment/collectors/metadata.py:35-68`, run from
//! `doctree-read` at priority 880) that removes that node again once it has
//! been read ([`crate::env::metadata`] is the reading).
//!
//! Sphinx runs DocInfo on a tree DocTitle never touched
//! (`doctitle_xform=False`, `sphinx/environment/__init__.py:69`): no section
//! title is promoted, so the field list it looks for is a child of the
//! document itself — a field list below a section's title is no docinfo.
//!
//! What a later read transform sees between the two: the docinfo, where
//! DocInfo inserted it, holding the bibliographic elements (`author`, ...,
//! whose children are the field's paragraph's), an `authors` element, and
//! every other field as it was, with a class. A footnote reference in it is
//! numbered, a reference in it resolved or reported (probed: `:author: Me
//! [#]_` collects `'Me 1'`).
//!
//! Only docutils' English language module (`docutils/languages/en.py`) is
//! ported: its bibliographic field names, the topics' titles and the author
//! separators. Sphinx hands DocInfo `config.language`'s module.

use super::references::is_text_element;
use super::TransformCtx;
use crate::doctree::ids::{fully_normalize_name, make_id};
use crate::doctree::{kinds, messages, AttrValue, Node, Span, RAWSOURCE};
use crate::env::metadata::metadata_from_docinfo;
use crate::rst::ParseOptions;
use crate::utils::{py_isspace, PY_DECIMAL_ZEROS};

const DOCINFO: &str = "docinfo";
const TOPIC: &str = "topic";

/// `isinstance(node, nodes.PreBibliographic)`: what DocInfo, and the
/// collector after it, look past for the document's first real child —
/// probed as the classes of docutils 0.22.4 and `sphinx.addnodes` deriving
/// from it: the `Invisible` ones (`comment`, `substitution_definition`,
/// `target`, `pending`, and Sphinx's `index`), `title`, `subtitle`, `meta`,
/// `decoration`, `system_message` and `raw`.
pub(crate) fn is_pre_bibliographic(node: &Node) -> bool {
    matches!(
        node.kind,
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

/// `isinstance(node, (nodes.Titular, nodes.decoration, nodes.meta))`
/// (`frontmatter.py:370-371`): what the docinfo is inserted after.
/// `Titular` is `title`, `subtitle` and `rubric`.
fn is_titular_decoration_or_meta(node: &Node) -> bool {
    matches!(
        node.kind,
        kinds::TITLE | kinds::SUBTITLE | "rubric" | "decoration" | "meta"
    )
}

/// The canonical name of a bibliographic field: `bibliographic_fields` of
/// docutils' English language module (`docutils/languages/en.py:42-55`),
/// keyed by the `fully_normalize_name`d field name, every canonical name
/// its own key. `biblio_nodes` (`frontmatter.py:344-356`) makes the first
/// ten elements of that name, `authors` an element of authors, and the
/// last two topics.
fn bibliographic_field(normalized: &str) -> Option<&'static str> {
    Some(match normalized {
        "author" => "author",
        "authors" => "authors",
        "organization" => "organization",
        "address" => "address",
        "contact" => "contact",
        "version" => "version",
        "revision" => "revision",
        "status" => "status",
        "date" => "date",
        "copyright" => "copyright",
        "dedication" => "dedication",
        "abstract" => "abstract",
        _ => return None,
    })
}

/// The bibliographic element classes that are `TextElement`s (probed:
/// all but `authors`) — what DocInfo fills with a field's single paragraph
/// (`frontmatter.py:390-395`) and the collector reads by class name.
pub(crate) fn is_bibliographic_text_element(kind: &str) -> bool {
    matches!(
        kind,
        "author"
            | "organization"
            | "address"
            | "contact"
            | "version"
            | "revision"
            | "status"
            | "date"
            | "copyright"
    )
}

/// `DocInfo.apply` (`frontmatter.py:360-374`): when the document's first
/// child that is not `PreBibliographic` is a field list, the field list is
/// replaced by what [`extract_bibliographic`] makes of it, inserted where
/// the document's leading titles end — ahead of any comment or target the
/// field list followed (probed: `.. c` + `:abstract: x` gives the abstract
/// topic, then the comment).
pub(super) fn doc_info(ctx: &mut TransformCtx) {
    let children = &ctx.tree.root.children;
    let Some(index) = children.iter().position(|node| !is_pre_bibliographic(node)) else {
        return;
    };
    if children[index].kind != kinds::FIELD_LIST {
        return;
    }
    // Every child before the field list is PreBibliographic, so the first
    // that is not titular is at the latest the field list itself: the
    // removal below cannot move it.
    let biblioindex = children
        .iter()
        .position(|node| !is_titular_decoration_or_meta(node))
        .unwrap_or(index);
    let field_list = ctx.tree.root.children.remove(index);
    let nodelist = extract_bibliographic(ctx, field_list);
    ctx.tree
        .root
        .children
        .splice(biblioindex..biblioindex, nodelist);
}

/// The span of every element DocInfo builds: none. docutils stamps an
/// inserted node with the document's `current_source`/`current_line`
/// (`setup_child`, `docutils/nodes.py:150-157`), but both are `None` once
/// the parse has finished (probed), and the elements inside the docinfo
/// are never set up at all — so a message about a node in a bibliographic
/// element is located where the reporter's state machine stopped
/// ([`TransformCtx::message_at`]; probed: a dangling reference in
/// `:author:` of a three-line document reports line 4, of one ending in a
/// list no line).
const UNSTAMPED: Span = Span::ZERO;

/// `DocInfo.extract_bibliographic` (`frontmatter.py:376-427`): every field
/// whose name is bibliographic, and well-formed, becomes its element in the
/// `docinfo` (or its topic); every other field goes into the `docinfo` as
/// it is, with the `make_id` of its normalized name as a class (and an RCS
/// keyword cleaned out of a single-paragraph body). The `docinfo`, if it
/// holds anything, then the dedication, then the abstract.
///
/// A field's name is its `field_name`'s FIRST child's text (`field[0][0]`,
/// `:383`): `:Author *x*: v` is an author.
fn extract_bibliographic(ctx: &mut TransformCtx, field_list: Node) -> Vec<Node> {
    let mut docinfo = Node::elem(DOCINFO, UNSTAMPED);
    let mut dedication: Option<Node> = None;
    let mut abstract_: Option<Node> = None;
    for mut field in field_list.children {
        let name = field
            .children
            .first()
            .and_then(|field_name| field_name.children.first())
            .map(astext)
            .unwrap_or_default();
        let normalized = fully_normalize_name(&name);
        let canonical = bibliographic_field(&normalized).filter(|_| {
            field.children.len() == 2 && check_empty_biblio_field(ctx, &mut field, &name)
        });
        let extracted = match canonical {
            None => false,
            Some(canonical) if is_bibliographic_text_element(canonical) => {
                if check_compound_biblio_field(ctx, &mut field, &name) {
                    let mut paragraph = field.children[1].children.remove(0);
                    clean_rcs_keywords(&mut paragraph);
                    let mut element = Node::elem(canonical, UNSTAMPED);
                    element.children = paragraph.children;
                    docinfo.children.push(element);
                    true
                } else {
                    false
                }
            }
            Some("authors") => extract_authors(ctx, &mut field, &name, &mut docinfo),
            Some(canonical) => {
                let slot = if canonical == "dedication" {
                    &mut dedication
                } else {
                    &mut abstract_
                };
                if slot.is_some() {
                    let text = format!("There can only be one \"{name}\" field.");
                    warn_into_body(ctx, &mut field, &text);
                    false
                } else {
                    *slot = Some(topic(canonical, &mut field));
                    true
                }
            }
        };
        if !extracted {
            if let Some(body) = field.children.last_mut() {
                if let [paragraph] = body.children.as_mut_slice() {
                    if paragraph.kind == kinds::PARAGRAPH {
                        clean_rcs_keywords(paragraph);
                    }
                }
            }
            let class = make_id(&normalized);
            if !class.is_empty() {
                field.attrs.classes.push(class);
            }
            docinfo.children.push(field);
        }
    }
    let mut nodelist = Vec::new();
    if !docinfo.children.is_empty() {
        nodelist.push(docinfo);
    }
    nodelist.extend(dedication);
    nodelist.extend(abstract_);
    nodelist
}

/// The dedication or abstract `topic` (`frontmatter.py:398-407`): class
/// `canonical`, a title with the language's label (English, `en.py:
/// 28-29`), and the field body's children.
fn topic(canonical: &str, field: &mut Node) -> Node {
    let label = if canonical == "dedication" {
        "Dedication"
    } else {
        "Abstract"
    };
    let mut title = Node::elem(kinds::TITLE, UNSTAMPED);
    title.children.push(Node::text_node(label, UNSTAMPED));
    let mut topic = Node::elem(TOPIC, UNSTAMPED);
    topic.attrs.classes.push(canonical.to_string());
    topic.children.push(title);
    topic.children.append(&mut field.children[1].children);
    topic
}

/// `self.document.reporter.warning(text, base_node=field)` appended to the
/// field's body (`field[-1] += …`): located at the field — its marker line.
fn warn_into_body(ctx: &mut TransformCtx, field: &mut Node, text: &str) {
    let message = ctx.message(
        messages::WARNING,
        text,
        field.span.source,
        Some(field.span.line),
    );
    ctx.reporter.report(&message);
    if let Some(body) = field.children.last_mut() {
        body.children.push(message);
    }
}

/// `check_empty_biblio_field` (`frontmatter.py:429-435`).
fn check_empty_biblio_field(ctx: &mut TransformCtx, field: &mut Node, name: &str) -> bool {
    if field
        .children
        .last()
        .is_some_and(|body| body.children.is_empty())
    {
        let text = format!("Cannot extract empty bibliographic field \"{name}\".");
        warn_into_body(ctx, field, &text);
        return false;
    }
    true
}

/// `check_compound_biblio_field` (`frontmatter.py:437-468`): the body must
/// be one paragraph. A body that is a one-line enumerated list — an author
/// with an initial, `:author: J. Doe` — is parsed again, escaped
/// (`'\\' + f_body.rawsource`), and kept as the paragraph that gives.
/// Otherwise the warning names what the body holds, its messages left out.
///
/// A body of messages alone has nothing to name: docutils indexes the empty
/// list (`content[0]`) and Sphinx's read aborts with an `IndexError` (`:463`;
/// probed: `:version:` over an unknown directive). This port names it as
/// the empty list, `[]`.
fn check_compound_biblio_field(ctx: &mut TransformCtx, field: &mut Node, name: &str) -> bool {
    let body = &mut field.children[1];
    if let [only] = body.children.as_slice() {
        if only.kind == kinds::PARAGRAPH {
            return true;
        }
    }
    let rawsource = match body.get(RAWSOURCE) {
        Some(AttrValue::Str(rawsource)) => Some(rawsource.clone()),
        _ => None,
    };
    if let Some(rawsource) = rawsource.filter(|rawsource| {
        body.children
            .first()
            .is_some_and(|first| first.kind == kinds::ENUMERATED_LIST)
            && !rawsource.trim_matches(py_isspace).contains('\n')
    }) {
        if let Some(paragraph) = reparse_escaped(ctx, &rawsource) {
            body.children = vec![paragraph];
            return true;
        }
    }
    let content: Vec<String> = body
        .children
        .iter()
        .filter(|child| child.kind != kinds::SYSTEM_MESSAGE)
        .map(|child| format!("<{}>", child.kind))
        .collect();
    let content = match content.as_slice() {
        [one] => format!("a {one}"),
        many => format!("[{}]", many.join(", ")),
    };
    let text = format!(
        "Bibliographic field \"{name}\"\nmust contain a single <paragraph>, not {content}."
    );
    warn_into_body(ctx, field, &text);
    false
}

/// The restoration parse (`frontmatter.py:446-456`): `'\\' + rawsource`
/// parsed into a new document (`'*DocInfo transform*'`, no transforms),
/// and its one child if that is a paragraph. That document's reporter
/// writes to the console directly, not to Sphinx's warning stream (probed:
/// `*DocInfo transform*:1: (WARNING/2) …` on stderr, not counted), so the
/// parse's records are dropped.
fn reparse_escaped(ctx: &TransformCtx, rawsource: &str) -> Option<Node> {
    let opts = ParseOptions {
        source_path: "*DocInfo transform*".to_string(),
        sphinx: true,
        docname: ctx.docname.to_string(),
        ..ParseOptions::default()
    };
    let mut children = crate::rst::parse_rst(&format!("\\{rawsource}"), &opts)
        .root
        .children;
    match children.as_slice() {
        [only] if only.kind == kinds::PARAGRAPH => children.pop(),
        _ => None,
    }
}

/// `extract_authors` (`frontmatter.py:479-508`): one paragraph of names
/// split at a separator, a bullet list of one-paragraph items, or several
/// paragraphs (comments skipped) — each nonempty name an `author` in an
/// `authors` element. Anything else warns into the body.
fn extract_authors(
    ctx: &mut TransformCtx,
    field: &mut Node,
    name: &str,
    docinfo: &mut Node,
) -> bool {
    let body = &field.children[1];
    let authors = match body.children.as_slice() {
        [paragraph] if paragraph.kind == kinds::PARAGRAPH => authors_from_one_paragraph(body),
        [list] if list.kind == kinds::BULLET_LIST => authors_from_bullet_list(list),
        [_] => None,
        _ => authors_from_paragraphs(body),
    };
    let author_nodes: Vec<Node> = authors
        .unwrap_or_default()
        .into_iter()
        .filter(|author| !author.is_empty())
        .map(|children| {
            let mut author = Node::elem("author", UNSTAMPED);
            author.children = children;
            author
        })
        .collect();
    if author_nodes.is_empty() {
        let text = format!(
            "Cannot extract \"{name}\" from bibliographic field:\n\
             Bibliographic field \"{name}\" must contain either\n \
             a single paragraph (with author names separated by a character \
             from the set \";,\"),\n \
             multiple paragraphs (one per author),\n \
             or a bullet list with one author name per item.\n\
             Note: Leading initials can cause (mis)recognizing names as \
             enumerated list."
        );
        warn_into_body(ctx, field, &text);
        return false;
    }
    let mut authors = Node::elem("authors", UNSTAMPED);
    authors.children = author_nodes;
    docinfo.children.push(authors);
    true
}

/// `authors_from_one_paragraph` (`frontmatter.py:510-528`): `str(node)`
/// of every `Text` in the body — escapes kept as `\x00`s — split at the
/// first of the language's author separators (English: `;`, then `,`;
/// `en.py:58`) that splits it, but never at an escaped one
/// (`(?<!\x00)`), each name stripped and made a Text again — markup is
/// not kept.
fn authors_from_one_paragraph(body: &Node) -> Option<Vec<Vec<Node>>> {
    let mut text = String::new();
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        match node.null_escaped() {
            Some(piece) => text.push_str(&piece),
            None => stack.extend(node.children.iter().rev()),
        }
    }
    if text.is_empty() {
        return None;
    }
    let mut names: Vec<&str> = Vec::new();
    for separator in [';', ','] {
        names = split_unescaped(&text, separator);
        if names.len() > 1 {
            break;
        }
    }
    Some(
        names
            .into_iter()
            .map(|name| name.trim_matches(py_isspace))
            .filter(|name| !name.is_empty())
            .map(|name| vec![Node::text_from_null_escaped(name, UNSTAMPED)])
            .collect(),
    )
}

/// `re.split('(?<!\x00)' + separator, text)`: `text` split at every
/// `separator` no `\x00` precedes.
fn split_unescaped(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut previous = None;
    for (at, c) in text.char_indices() {
        if c == separator && previous != Some('\0') {
            parts.push(&text[start..at]);
            start = at + c.len_utf8();
        }
        previous = Some(c);
    }
    parts.push(&text[start..]);
    parts
}

/// `authors_from_bullet_list` (`frontmatter.py:530-540`).
fn authors_from_bullet_list(list: &Node) -> Option<Vec<Vec<Node>>> {
    let mut authors = Vec::new();
    for item in &list.children {
        if item.kind == kinds::COMMENT {
            continue;
        }
        match item.children.as_slice() {
            [paragraph] if paragraph.kind == kinds::PARAGRAPH => {
                authors.push(paragraph.children.clone());
            }
            _ => return None,
        }
    }
    (!authors.is_empty()).then_some(authors)
}

/// `authors_from_paragraphs` (`frontmatter.py:542-548`).
fn authors_from_paragraphs(body: &Node) -> Option<Vec<Vec<Node>>> {
    if body
        .children
        .iter()
        .any(|item| !matches!(item.kind, kinds::PARAGRAPH | kinds::COMMENT))
    {
        return None;
    }
    Some(
        body.children
            .iter()
            .filter(|item| item.kind != kinds::COMMENT)
            .map(|item| item.children.clone())
            .collect(),
    )
}

lazy_static::lazy_static! {
    /// `DocInfo.rcs_keyword_substitutions` (`frontmatter.py:470-477`), in
    /// order. The first two are `re.IGNORECASE`, spelled out here as the
    /// characters Python's `re` matches each letter with (probed over every
    /// code point: `s` also matches `ſ`, `i` also `İ` and `ı`, no other
    /// letter anything beyond its two cases); `\d` is Python's — the
    /// Unicode 15.0 decimal digits ([`PY_DECIMAL_ZEROS`]).
    static ref RCS_KEYWORDS: [(regex::Regex, &'static str); 3] = {
        let digit = python_digit_class();
        [
            (
                regex::Regex::new(&format!(
                    r"\$[Dd][Aa][Tt][Ee]: ({digit}{digit}{digit}{digit})[-/]({digit}{digit})[-/]({digit}{digit})[ Tt](?:{digit}|:)+[^$]* \$"
                ))
                .expect("the RCS Date pattern compiles"),
                "${1}-${2}-${3}",
            ),
            (
                regex::Regex::new(
                    r"\$[Rr][Cc][Ss\x{17F}][Ff][Ii\x{130}\x{131}][Ll][Ee]: (.+),[Vv] \$",
                )
                .expect("the RCS RCSfile pattern compiles"),
                "${1}",
            ),
            (
                regex::Regex::new(r"\$[a-zA-Z]+: (.+) \$").expect("the RCS keyword pattern compiles"),
                "${1}",
            ),
        ]
    };
}

/// Python `re`'s `\d` on a `str` pattern, as a character class.
fn python_digit_class() -> String {
    let mut class = String::from("[");
    for zero in PY_DECIMAL_ZEROS {
        class.push_str(&format!(r"\x{{{zero:X}}}-\x{{{:X}}}", zero + 9));
    }
    class.push(']');
    class
}

/// `docutils.utils.clean_rcs_keywords` (`utils/__init__.py:497-507`): a
/// paragraph that is a single `Text` has the first RCS keyword pattern
/// that matches it substituted throughout — `$Date: 2026/09/30 12:00:00 $`
/// becomes `2026-09-30`, `$RCSfile: x.py,v $` `x.py`, `$Keyword: text $`
/// `text`. The patterns run on `str(textnode)` and `nodes.Text` takes what
/// they give, escapes and all.
fn clean_rcs_keywords(paragraph: &mut Node) {
    let [text] = paragraph.children.as_mut_slice() else {
        return;
    };
    let Some(value) = text.null_escaped() else {
        return;
    };
    for (pattern, substitution) in RCS_KEYWORDS.iter() {
        if pattern.is_match(&value) {
            let cleaned = pattern.replace_all(&value, *substitution).into_owned();
            *text = Node::text_from_null_escaped(&cleaned, text.span);
            return;
        }
    }
}

/// The removal half of `MetadataCollector.process_doc`
/// (`collectors/metadata.py:40-43,68`), at its `doctree-read` slot (880):
/// when the document's first child that is not `PreBibliographic` is a
/// `docinfo`, it is read ([`metadata_from_docinfo`]) and popped. The
/// dedication and abstract topics stay. Before FilterSystemMessages (999):
/// what the collector reads still holds DocInfo's messages, whatever
/// `keep_warnings` says (probed).
pub(super) fn metadata_collector(ctx: &mut TransformCtx) {
    let children = &mut ctx.tree.root.children;
    let Some(index) = children.iter().position(|node| !is_pre_bibliographic(node)) else {
        return;
    };
    if children[index].kind != DOCINFO {
        return;
    }
    let docinfo = children.remove(index);
    ctx.metadata = metadata_from_docinfo(&docinfo);
}

/// docutils' `Node.astext()` (`docutils/nodes.py:440-441,746-748`): a
/// text's own text, an element's children's joined by its
/// `child_text_separator` — `''` for a `TextElement` (and Sphinx's
/// `pending_xref`, `option`), `', '` for an `option_group`, `'  '` for an
/// `option_list_item`, a blank line for any other — with the classes that
/// override it: `system_message` (its location prefix, `:2516-2519`),
/// `image` (its `alt`, `:2382-2383`), `option_argument` (its delimiter,
/// `:2268-2269`), and Sphinx's signature parts (`sphinx/addnodes.py:
/// 237-290`). By an explicit stack: no depth of nesting overflows.
pub(crate) fn astext(node: &Node) -> String {
    struct Frame<'n> {
        node: &'n Node,
        next: usize,
        parts: Vec<String>,
    }
    if let Some(text) = &node.text {
        return text.clone();
    }
    let mut stack = vec![Frame {
        node,
        next: 0,
        parts: Vec::new(),
    }];
    while let Some(frame) = stack.last_mut() {
        if let Some(child) = frame.node.children.get(frame.next) {
            frame.next += 1;
            match &child.text {
                Some(text) => frame.parts.push(text.clone()),
                None => stack.push(Frame {
                    node: child,
                    next: 0,
                    parts: Vec::new(),
                }),
            }
            continue;
        }
        let Frame { node, parts, .. } = stack.pop().expect("the frame just looked at");
        let text = element_text(node, parts);
        match stack.last_mut() {
            Some(parent) => parent.parts.push(text),
            None => return text,
        }
    }
    String::new()
}

/// One element's `astext()` from its children's ([`astext`]).
fn element_text(node: &Node, parts: Vec<String>) -> String {
    let attr = |key: &'static str| match node.get(key) {
        Some(AttrValue::Str(value)) => value.clone(),
        Some(AttrValue::Int(value)) => value.to_string(),
        _ => String::new(),
    };
    match node.kind {
        kinds::SYSTEM_MESSAGE => format!(
            "{}:{}: ({}/{}) {}",
            attr("source"),
            attr("line"),
            attr("type"),
            attr("level"),
            parts.join("\n\n")
        ),
        kinds::IMAGE => attr("alt"),
        kinds::OPTION_ARGUMENT => {
            let delimiter = match node.get("delimiter") {
                Some(AttrValue::Str(delimiter)) => delimiter.as_str(),
                _ => " ",
            };
            format!("{delimiter}{}", parts.concat())
        }
        "desc_returns" => format!(" -> {}", parts.concat()),
        "desc_parameterlist" => format!("({})", parts.join(", ")),
        "desc_type_parameter_list" | "desc_optional" => format!("[{}]", parts.join(", ")),
        kinds::OPTION_GROUP => parts.join(", "),
        kinds::OPTION_LIST_ITEM => parts.join("  "),
        kinds::OPTION | "pending_xref" => parts.concat(),
        kind if is_text_element(kind) => parts.concat(),
        _ => parts.join("\n\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transforms::{parse_and_transform, TransformConfig};

    fn opts() -> ParseOptions {
        ParseOptions {
            source_path: "<snippet>".to_string(),
            sphinx: true,
            docname: "index".to_string(),
            ..Default::default()
        }
    }

    /// The tree DocInfo leaves for the collector: run the pass with only
    /// DocInfo (and what precedes it) registered.
    fn docinfo_tree(source: &str) -> Node {
        let mut out = crate::rst::parse_rst_full(source, &opts());
        let config = TransformConfig::default();
        let mut ctx = TransformCtx::new(
            &mut out.doctree,
            out.ids,
            out.next_seq,
            out.end_of_input,
            "index",
            &config,
        );
        ctx.run(&[(340, "DocInfo", doc_info as fn(&mut TransformCtx))]);
        out.doctree.root
    }

    /// The docinfo's shape before the collector pops it (docutils 0.22.4,
    /// `publish_doctree` with `doctitle_xform=False`, probed): elements for
    /// the bibliographic fields, the generic field kept with its `make_id`
    /// class, the topics after the docinfo.
    #[test]
    fn the_docinfo_holds_elements_fields_and_is_followed_by_the_topics() {
        let root = docinfo_tree(
            ":Author: Me\n:authors: A; B\n:Custom Field: v\n:abstract: Sum.\n\nBody.\n",
        );
        let pformat = root.pformat();
        assert_eq!(
            pformat,
            "<document source=\"<snippet>\">\n\
             \x20   <docinfo>\n\
             \x20       <author>\n\
             \x20           Me\n\
             \x20       <authors>\n\
             \x20           <author>\n\
             \x20               A\n\
             \x20           <author>\n\
             \x20               B\n\
             \x20       <field classes=\"custom-field\">\n\
             \x20           <field_name>\n\
             \x20               Custom Field\n\
             \x20           <field_body>\n\
             \x20               <paragraph>\n\
             \x20                   v\n\
             \x20   <topic classes=\"abstract\">\n\
             \x20       <title>\n\
             \x20           Abstract\n\
             \x20       <paragraph>\n\
             \x20           Sum.\n\
             \x20   <paragraph>\n\
             \x20       Body.\n"
        );
    }

    /// The docinfo and the elements in it carry no line, as in docutils:
    /// a message about a node in them is located by the reporter's
    /// fallback ([`TransformCtx::message_at`]).
    #[test]
    fn the_docinfo_elements_carry_no_line() {
        let root = docinfo_tree(":author: x\n\nBody.\n");
        let docinfo = &root.children[0];
        assert_eq!(docinfo.kind, DOCINFO);
        assert_eq!((docinfo.span.line, docinfo.children[0].span.line), (0, 0));
    }

    /// A bibliographic field whose body holds only messages: Sphinx's read
    /// aborts (`IndexError`, `frontmatter.py:463`; probed); the port names
    /// the empty content `[]` and carries on.
    #[test]
    fn a_body_of_messages_alone_is_named_as_the_empty_list() {
        let (tree, records) = parse_and_transform(
            ":version:\n   .. nosuch::\n\nBody.\n",
            &opts(),
            &TransformConfig::default(),
        );
        let texts: Vec<&str> = records.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Unknown directive type \"nosuch\".\n\n.. nosuch::",
                "Bibliographic field \"version\"\nmust contain a single <paragraph>, not [].",
            ]
        );
        let kinds: Vec<&str> = tree.root.children.iter().map(|n| n.kind).collect();
        assert_eq!(kinds, [kinds::PARAGRAPH]);
    }

    /// The collector pops the docinfo and keeps the topics.
    #[test]
    fn the_collector_removes_the_docinfo_only() {
        let (tree, _) = parse_and_transform(
            ":orphan:\n:dedication: D\n\nBody.\n",
            &opts(),
            &TransformConfig::default(),
        );
        let kinds: Vec<&str> = tree.root.children.iter().map(|n| n.kind).collect();
        assert_eq!(kinds, [TOPIC, kinds::PARAGRAPH]);
    }

    /// RCS keywords (`frontmatter.py:470-477`): the date reformatted, the
    /// `,v` suffix dropped, any other keyword's dollar framing removed —
    /// throughout the text, by the first pattern that matches; with
    /// Python's case-insensitive letters.
    #[test]
    fn rcs_keywords_are_cleaned_like_docutils() {
        let clean = |text: &str| {
            let mut paragraph = Node::elem(kinds::PARAGRAPH, Span::ZERO);
            paragraph.children.push(Node::text_node(text, Span::ZERO));
            clean_rcs_keywords(&mut paragraph);
            paragraph.astext()
        };
        assert_eq!(clean("$Date: 2026/09/30 12:00:00 $"), "2026-09-30");
        assert_eq!(clean("$date: 2026-09-30T12:00:00 $"), "2026-09-30");
        assert_eq!(
            clean("$Date: \u{663}\u{663}\u{663}\u{663}/01/02 1 $"),
            "\u{663}\u{663}\u{663}\u{663}-01-02"
        );
        assert_eq!(clean("$RCSfile: frontmatter.py,v $"), "frontmatter.py");
        assert_eq!(clean("$RC\u{17F}f\u{131}le: a.py,V $"), "a.py");
        assert_eq!(clean("a $Revision: 1.2 $ b"), "a 1.2 b");
        assert_eq!(clean("no keyword"), "no keyword");
        assert_eq!(clean("$Id:x $"), "$Id:x $");
    }

    /// `pattern.sub(substitution, textnode)` (`docutils/utils/__init__.py:
    /// 497-507`) runs on `str(textnode)`, and `nodes.Text` keeps what it
    /// gives: the escapes come through the substitution (probed:
    /// `:date: $Date: x\* $` is `'x\x00*'`).
    #[test]
    fn rcs_keywords_are_cleaned_on_the_null_escaped_text() {
        let mut paragraph = Node::elem(kinds::PARAGRAPH, Span::ZERO);
        paragraph
            .children
            .push(Node::text_from_null_escaped("$Id: a\u{0}*b $", Span::ZERO));
        clean_rcs_keywords(&mut paragraph);
        assert_eq!(
            paragraph.children[0].null_escaped().as_deref(),
            Some("a\u{0}*b")
        );
    }

    /// `authors_from_one_paragraph` (`frontmatter.py:510-528`) joins the
    /// Text nodes' `str(node)`, splits with `(?<!\x00)` before the
    /// separator and strips each name, nulls and all (probed: `A\; B` is
    /// one author `'A\x00; B'`; `A ;\  B` two, the second `'\x00  B'`).
    #[test]
    fn an_escaped_separator_does_not_split_authors() {
        let authors = |source: &str| {
            let root = docinfo_tree(source);
            let authors = root.children[0]
                .children
                .iter()
                .find(|node| node.kind == "authors")
                .expect("an authors element");
            authors
                .children
                .iter()
                .map(|author| author.children[0].null_escaped().unwrap().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(authors(":authors: A\\; B\n\nBody.\n"), ["A\u{0}; B"]);
        assert_eq!(authors(":authors: A\\, B\n\nBody.\n"), ["A\u{0}, B"]);
        assert_eq!(
            authors(":authors: A\\; B, C\n\nBody.\n"),
            ["A\u{0}; B", "C"]
        );
        assert_eq!(authors(":authors: A ;\\  B\n\nBody.\n"), ["A", "\u{0}  B"]);
    }

    /// `astext()` separators and overrides, as docutils joins them.
    #[test]
    fn astext_follows_docutils_separators() {
        let mut body = Node::elem(kinds::FIELD_BODY, Span::ZERO);
        let mut paragraph = Node::elem(kinds::PARAGRAPH, Span::ZERO);
        paragraph.children.push(Node::text_node("a ", Span::ZERO));
        let mut emphasis = Node::elem(kinds::EMPHASIS, Span::ZERO);
        emphasis.children.push(Node::text_node("b", Span::ZERO));
        paragraph.children.push(emphasis);
        body.children.push(paragraph);
        let mut image = Node::elem(kinds::IMAGE, Span::ZERO);
        image.set("alt", AttrValue::Str("pic".to_string()));
        body.children.push(image);
        body.children.push(messages::system_message(
            messages::WARNING,
            "Oops.",
            0,
            3,
            "<snippet>",
        ));
        assert_eq!(
            astext(&body),
            "a b\n\npic\n\n<snippet>:3: (WARNING/2) Oops."
        );
    }
}
