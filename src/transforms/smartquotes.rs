//! SphinxSmartQuotes (priority 750, `sphinx/transforms/__init__.py:
//! 361-415`): docutils' `SmartQuotes.apply` (`docutils/transforms/
//! universal.py:280-340`) with Sphinx's availability test (`:382-401`), its
//! `smartquotes_action` (`:370-380`) and its `get_tokens` (`:403-415`,
//! `sphinx/util/nodes.py:697-716`), over docutils' SmartyPants port
//! (`docutils/utils/smartquotes.py:565-880`), ported here function by
//! function.
//!
//! The regular expressions of `educateQuotes` (`smartquotes.py:508-555`)
//! need lookbehind and lookahead, which the `regex` crate has not: each is
//! an explicit scanner with Python `re`'s `str` semantics — every
//! substitution one left-to-right pass over the string the previous one
//! left, matches never overlapping, lookarounds reading that pass's input —
//! over Python's character classes (`\w`, `\s`, `\d` and the module's own,
//! [`super::smartquotes_tables`]).
//!
//! It runs before SphinxDomains (850) and the collectors (880), so the
//! environment reads the educated text: titles, tocs, label section names,
//! glossary terms. What the parse computed from the raw text — ids, names,
//! index entries, a toctree's caption attribute — stays as written.

use std::collections::BTreeSet;

use super::references::{collect_paths, is_text_element};
use super::smartquotes_tables as tables;
use super::{node_at, node_at_mut, NodePath, TransformConfig, TransformCtx};
use crate::doctree::{kinds, messages, AttrValue, Node};

/// The transform. A unit is every `TextElement` in document order
/// (`findall(nodes.TextElement)`) that is neither a `FixedTextElement` nor
/// `Special` (`nodes_to_skip`, `universal.py:248,297-300`) and whose parent
/// is no `TextElement` (`:301-303`) — so an inline under a non-text element
/// (an xref's `inline` under its `pending_xref`) is a unit of its own,
/// educated a second time after its paragraph. Its tokens are its Text
/// descendants but those of an `option_string` (`:305-308`), educated
/// together in its language (`:310-331`); each token's educated text then
/// replaces the first Text sibling equal to its node — not always the node
/// itself ([`replace_first_equal`], `:333-338`).
pub(crate) fn smart_quotes(ctx: &mut TransformCtx) {
    if !is_available(ctx.config) {
        return;
    }
    let config = ctx.config;
    // `self.unsupported_languages`, cleared after each document (`:340`).
    let mut unsupported: BTreeSet<String> = BTreeSet::new();
    for path in collect_paths(&ctx.tree.root, |node| is_text_element(node.kind)) {
        let root = &ctx.tree.root;
        let chain = ancestry(root, &path);
        let Some((unit, ancestors)) = chain.split_last() else {
            continue;
        };
        if is_fixed_text_element(unit.kind) || is_special(unit.kind) {
            continue;
        }
        if ancestors
            .last()
            .is_some_and(|parent| is_text_element(parent.kind))
        {
            continue;
        }
        let code = language_code(&chain, &config.language);
        let language = match quotes_tag(code) {
            Some(tag) => tag,
            None => {
                // "language not supported -- keep ASCII quotes" (`:323-330`):
                // a loose WARNING at the unit (`base_node=node`), once per
                // language per document.
                if unsupported.insert(code.to_string()) {
                    let text = format!("No smart quotes defined for language \"{code}\".");
                    let located = chain
                        .iter()
                        .rev()
                        .find(|node| node.span.line != 0)
                        .map_or((0, 0), |node| (node.span.source, node.span.line));
                    let warning = ctx.message_at(messages::WARNING, &text, located);
                    ctx.reporter.report(&warning);
                }
                ""
            }
        };
        // `is_smartquotable` reads every ancestor of a Text up to the root:
        // those above the unit (and the unit) are the same for all of them.
        let outer_literal = chain.iter().any(|node| non_smartquotable(node));
        let Some(unit) = node_at_mut(&mut ctx.tree.root, &path) else {
            continue;
        };
        let texts = unit_texts(unit, outer_literal);
        // Each Text's `str()`, which the replacement below compares.
        let originals: Vec<String> = texts
            .iter()
            .map(|(at, _)| {
                node_at(unit, at)
                    .and_then(Node::null_escaped)
                    .unwrap_or_default()
                    .into_owned()
            })
            .collect();
        let tokens: Vec<(TokenKind, String)> = texts
            .iter()
            .zip(&originals)
            .map(|((at, literal), original)| {
                if *literal {
                    let text = node_at(unit, at).map(Node::astext).unwrap_or_default();
                    (TokenKind::Literal, text)
                } else {
                    (TokenKind::Plain, backslash_escaped(original))
                }
            })
            .collect();
        let educated = educate_tokens(tokens, &config.smartquotes_action, language);
        for (((at, literal), original), new) in texts.iter().zip(&originals).zip(educated) {
            replace_first_equal(unit, at, original, |span| {
                // `nodes.Text(newtext)`: a literal token's is its
                // `astext()`, which holds no null; a plain one keeps every
                // null it had.
                if *literal {
                    Node::text_node(new, span)
                } else {
                    Node::text_from_null_escaped(&new, span)
                }
            });
        }
    }
}

/// `txtnode.parent.replace(txtnode, nodes.Text(newtext))`
/// (`universal.py:338`) for the Text at `at` below `unit`, whose `str()`
/// was `original`. `Element.replace` finds its child with `Element.index`
/// — `list.index`, the first child that IS the node or EQUALS it — and a
/// docutils `Text` is a `str`, equal by value, nulls included, while an
/// element equals only itself (`docutils/nodes.py:405,809-810,1101-1108`;
/// no node class defines `__eq__`). So the child replaced is the first
/// Text sibling whose `str()` is `original`: perhaps an earlier one an
/// earlier token rebuilt into that very string, the node itself then
/// keeping its old text — for a later token, or a nested unit's pass, to
/// find. One always lies at or before the node's own slot: every equal
/// text before it took at most one of the equal originals up to it. Each
/// call scans the siblings up to that slot, so a unit of `k` Text
/// siblings costs `O(k²)` — as upstream's `list.index` does.
fn replace_first_equal(
    unit: &mut Node,
    at: &[usize],
    original: &str,
    new: impl FnOnce(crate::doctree::Span) -> Node,
) {
    let Some((&own, parent_path)) = at.split_last() else {
        return;
    };
    let Some(parent) = node_at_mut(unit, parent_path) else {
        return;
    };
    let slot = parent
        .children
        .iter()
        .take(own + 1)
        .position(|child| {
            child.kind == kinds::TEXT && child.null_escaped().as_deref() == Some(original)
        })
        .unwrap_or(own);
    if let Some(child) = parent.children.get_mut(slot) {
        *child = new(child.span);
    }
}

/// `SphinxSmartQuotes.is_available` (`transforms/__init__.py:382-401`).
/// The document's `smart_quotes` setting is Sphinx's `True`
/// (`environment/__init__.py:382`), so the first test always passes.
fn is_available(config: &TransformConfig) -> bool {
    config.smartquotes
        && !config
            .smartquotes_excludes
            .builders
            .contains(&config.builder)
        && !config
            .smartquotes_excludes
            .languages
            .contains(&config.language)
        && quotes_tag(&config.language).is_some()
}

/// The nodes from the root down to the one at `path`, inclusive: one walk
/// down, so a unit costs its depth.
fn ancestry<'n>(root: &'n Node, path: &[usize]) -> Vec<&'n Node> {
    let mut chain = Vec::with_capacity(path.len() + 1);
    let mut node = root;
    chain.push(node);
    for &index in path {
        let Some(child) = node.children.get(index) else {
            break;
        };
        node = child;
        chain.push(node);
    }
    chain
}

/// `node.get_language_code(fallback)` (`docutils/nodes.py:773-786`) for the
/// last node of `chain`: the first `language-` class of the node, else of
/// its nearest ancestor that has one, else `fallback`.
fn language_code<'n>(chain: &[&'n Node], fallback: &'n str) -> &'n str {
    chain
        .iter()
        .rev()
        .find_map(|node| {
            node.attrs
                .classes
                .iter()
                .find_map(|class| class.strip_prefix("language-"))
        })
        .unwrap_or(fallback)
}

/// The unit's tokens (`:305-308`): the path below `unit` of every Text in
/// document order but those whose parent is an `option_string`, each with
/// whether `is_smartquotable` refuses it (`sphinx/util/nodes.py:708-716`)
/// — a non-smartquotable node among its ancestors, `outer_literal`
/// standing for the unit's own and those above it. An explicit stack of
/// sibling cursors: no nesting depth overflows the call stack.
fn unit_texts(unit: &Node, outer_literal: bool) -> Vec<(NodePath, bool)> {
    let mut texts = Vec::new();
    // Each frame: a sibling list, the next index in it, and what its
    // parent hands down; `path` is the path of that parent.
    let mut frames = vec![(
        unit.children.as_slice(),
        0,
        outer_literal,
        unit.kind == kinds::OPTION_STRING,
    )];
    let mut path: NodePath = Vec::new();
    while let Some(frame) = frames.last_mut() {
        let (siblings, index, literal, in_option_string) = *frame;
        let Some(child) = siblings.get(index) else {
            frames.pop();
            path.pop();
            continue;
        };
        frame.1 += 1;
        if child.kind == kinds::TEXT {
            if !in_option_string {
                let mut at = path.clone();
                at.push(index);
                texts.push((at, literal));
            }
        } else {
            path.push(index);
            frames.push((
                child.children.as_slice(),
                0,
                literal || non_smartquotable(child),
                child.kind == kinds::OPTION_STRING,
            ));
        }
    }
    texts
}

/// One step of `is_smartquotable` (`sphinx/util/nodes.py:697-714`): an
/// ancestor that makes a Text below it a literal token — a
/// `NON_SMARTQUOTABLE_PARENT_NODES` instance (`FixedTextElement`,
/// `literal`, `math`, `image`, `raw`, `problematic`, `not_smartquotable`)
/// or one whose `support_smartquotes` attribute is `False` (stored as `0`).
fn non_smartquotable(node: &Node) -> bool {
    is_fixed_text_element(node.kind)
        || matches!(
            node.kind,
            kinds::LITERAL | kinds::MATH | kinds::IMAGE | "raw" | kinds::PROBLEMATIC
        )
        || is_not_smartquotable(node.kind)
        || matches!(node.get("support_smartquotes"), Some(AttrValue::Int(0)))
}

/// `isinstance(node, nodes.FixedTextElement)` by tagname, as probed from
/// docutils 0.22.4 / Sphinx 9.1.0 (research
/// `2026-09-30-m2-wave5-transforms.md` Appendix A).
fn is_fixed_text_element(kind: &str) -> bool {
    matches!(
        kind,
        "address"
            | "comment"
            | "desc_addname"
            | "desc_annotation"
            | "desc_name"
            | "desc_optional"
            | "desc_parameter"
            | "desc_parameterlist"
            | "desc_returns"
            | "desc_signature_line"
            | "desc_type"
            | "desc_type_parameter"
            | "desc_type_parameter_list"
            | "doctest_block"
            | "literal_block"
            | "manpage"
            | "math_block"
            | "production"
            | "raw"
    )
}

/// `isinstance(node, nodes.Special)` (probed as for
/// [`is_fixed_text_element`]).
fn is_special(kind: &str) -> bool {
    matches!(
        kind,
        "comment"
            | "index"
            | "pending"
            | "raw"
            | "substitution_definition"
            | "system_message"
            | "target"
    )
}

/// `isinstance(node, addnodes.not_smartquotable)` (`sphinx/addnodes.py`;
/// probed as for [`is_fixed_text_element`]).
fn is_not_smartquotable(kind: &str) -> bool {
    matches!(
        kind,
        "desc_addname"
            | "desc_inline"
            | "desc_name"
            | "desc_signature"
            | "literal_emphasis"
            | "literal_strong"
    ) || kind.starts_with("desc_sig_")
}

/// Sphinx's plain token (`transforms/__init__.py:409-412`): `str(txtnode)`
/// with a backslash before each of `` -\'". ` `` that follows a null —
/// `re.sub(r'(?<=\x00)([-\\\'".`])', r'\\\1', ...)` — so that
/// `processEscapes` keeps an escaped character plain.
fn backslash_escaped(null_escaped: &str) -> String {
    let mut out = String::with_capacity(null_escaped.len());
    let mut after_null = false;
    for c in null_escaped.chars() {
        if after_null && matches!(c, '-' | '\\' | '\'' | '"' | '.' | '`') {
            out.push('\\');
        }
        out.push(c);
        after_null = c == '\0';
    }
    out
}

/// The first tag of `normalize_language_tag(tag)` (`docutils/utils/
/// __init__.py:741-765`) that `smartchars.quotes` has — the loop of
/// `universal.py:319-322` and the test of `transforms/__init__.py:400-401`.
///
/// `normalize_language_tag` lists every combination of the subtags, most
/// subtags first, combinations in `itertools` order, the bare base last:
/// exponential in the subtags. No table tag holds more than
/// [`tables::MAX_TAG_HYPHENS`] hyphens, so no combination of more subtags
/// can be one; for the others, each table tag is matched against the
/// subtags directly — at each count, the tag whose earliest choice of
/// subtags comes first in `itertools.combinations`' lexicographic index
/// order is the one the list names first.
pub(crate) fn quotes_tag(tag: &str) -> Option<&'static str> {
    let (base, subtags) = split_language_tag(tag);
    let subtags: Vec<&str> = subtags.iter().map(String::as_str).collect();
    for count in (1..=subtags.len().min(tables::MAX_TAG_HYPHENS)).rev() {
        if let Some(found) = first_combination(&base, &subtags, count) {
            return Some(found);
        }
    }
    quotes_entry(&base).map(|(tag, _)| tag)
}

/// `normalize_language_tag`'s normalization (`__init__.py:754-758`): the
/// tag lowercased, `-` read as `_`, a one-character subtag joined to the
/// next with `-` (`re.sub(r'_([a-zA-Z0-9])_', r'_\1-', tag)`), then split
/// at `_` into the base and the subtags.
fn split_language_tag(tag: &str) -> (String, Vec<String>) {
    let lowered: Vec<char> = tag.to_lowercase().replace('-', "_").chars().collect();
    let mut joined = String::with_capacity(lowered.len());
    let mut at = 0;
    while at < lowered.len() {
        if lowered[at] == '_'
            && lowered.get(at + 1).is_some_and(char::is_ascii_alphanumeric)
            && lowered.get(at + 2) == Some(&'_')
        {
            joined.push('_');
            joined.push(lowered[at + 1]);
            joined.push('-');
            at += 3;
        } else {
            joined.push(lowered[at]);
            at += 1;
        }
    }
    let mut parts = joined.split('_').map(str::to_string);
    let base = parts.next().unwrap_or_default();
    (base, parts.collect())
}

/// Among the table tags `base` + `-` + `count` of `subtags` joined with
/// `-`, the one whose lexicographically first index combination is the
/// smallest. A table tag's tail splits into `count` pieces at `count - 1`
/// of its hyphens (a subtag may hold a hyphen, `x-altquot`); for one split,
/// the first combination naming those pieces is the greedy earliest one.
fn first_combination(base: &str, subtags: &[&str], count: usize) -> Option<&'static str> {
    let mut best: Option<(Vec<usize>, &'static str)> = None;
    for (tag, _) in tables::QUOTES {
        let Some(tail) = tag
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix('-'))
        else {
            continue;
        };
        let hyphens: Vec<usize> = tail.match_indices('-').map(|(at, _)| at).collect();
        for cuts in index_combinations(hyphens.len(), count - 1) {
            let mut pieces = Vec::with_capacity(count);
            let mut start = 0;
            for cut in cuts {
                pieces.push(&tail[start..hyphens[cut]]);
                start = hyphens[cut] + 1;
            }
            pieces.push(&tail[start..]);
            let Some(indices) = earliest_indices(subtags, &pieces) else {
                continue;
            };
            if best.as_ref().is_none_or(|(seen, _)| indices < *seen) {
                best = Some((indices, tag));
            }
        }
    }
    best.map(|(_, tag)| tag)
}

/// Every `choose`-element subset of `0..from`, as sorted index lists.
fn index_combinations(from: usize, choose: usize) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut current = Vec::with_capacity(choose);
    fn extend(
        from: usize,
        choose: usize,
        next: usize,
        current: &mut Vec<usize>,
        out: &mut Vec<Vec<usize>>,
    ) {
        if current.len() == choose {
            out.push(current.clone());
            return;
        }
        for index in next..from {
            current.push(index);
            extend(from, choose, index + 1, current, out);
            current.pop();
        }
    }
    extend(from, choose, 0, &mut current, &mut out);
    out
}

/// The earliest increasing indices at which `subtags` spell `pieces`, if
/// any.
fn earliest_indices(subtags: &[&str], pieces: &[&str]) -> Option<Vec<usize>> {
    let mut indices = Vec::with_capacity(pieces.len());
    let mut from = 0;
    for piece in pieces {
        let at = from + subtags[from..].iter().position(|subtag| subtag == piece)?;
        indices.push(at);
        from = at + 1;
    }
    Some(indices)
}

/// `smartchars.quotes[tag]`.
fn quotes_entry(tag: &str) -> Option<(&'static str, [&'static str; 4])> {
    tables::QUOTES
        .binary_search_by(|(key, _)| (*key).cmp(tag))
        .ok()
        .map(|at| tables::QUOTES[at])
}

/// `smartchars(language)` (`smartquotes.py:499-505`): the language's
/// opening primary, closing primary, opening secondary and closing
/// secondary quote — ASCII for a language the table lacks.
fn smartchars(language: &str) -> [&'static str; 4] {
    quotes_entry(&language.to_lowercase()).map_or(tables::ASCII_QUOTES, |(_, quotes)| quotes)
}

/// The two kinds of token `educate_tokens` reads (`'tag'` never occurs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenKind {
    /// Text to educate.
    Plain,
    /// Text kept as it is, whose last character is still context.
    Literal,
}

/// What `educate_tokens`' `attr` turns on (`smartquotes.py:567-617`).
#[derive(Debug, Default)]
struct Actions {
    quotes: bool,
    /// 0 off, 1 ``` ``double'' ```, 2 also `` `single' ``.
    backticks: u8,
    /// 0 off, 1 `educateDashes`, 2 old school, 3 old school inverted.
    dashes: u8,
    ellipses: bool,
    convert_quot: bool,
    stupefy: bool,
}

impl Actions {
    /// `'1'`, `'2'`, `'3'` and `'-1'` name a whole setting; anything else
    /// is read letter by letter, a later letter winning (`:590-617`).
    fn from_attr(attr: &str) -> Actions {
        let everything = |dashes| Actions {
            quotes: true,
            backticks: 1,
            dashes,
            ellipses: true,
            ..Actions::default()
        };
        match attr {
            "1" => everything(1),
            "2" => everything(2),
            "3" => everything(3),
            "-1" => Actions {
                stupefy: true,
                ..Actions::default()
            },
            _ => {
                let mut actions = Actions::default();
                let has = |letter| attr.contains(letter);
                actions.quotes = has('q');
                if has('b') {
                    actions.backticks = 1;
                }
                if has('B') {
                    actions.backticks = 2;
                }
                if has('d') {
                    actions.dashes = 1;
                }
                if has('D') {
                    actions.dashes = 2;
                }
                if has('i') {
                    actions.dashes = 3;
                }
                actions.ellipses = has('e');
                actions.convert_quot = has('w');
                actions
            }
        }
    }
}

/// `educate_tokens(text_tokens, attr, language)` (`smartquotes.py:565-
/// 675`): each token's educated text, in order. A literal token passes
/// unchanged but its last character becomes the context of the next; the
/// first plain token's context is a space.
pub(crate) fn educate_tokens(
    tokens: Vec<(TokenKind, String)>,
    attr: &str,
    language: &str,
) -> Vec<String> {
    let actions = Actions::from_attr(attr);
    let mut previous_last = String::from(" ");
    let mut out = Vec::with_capacity(tokens.len());
    for (kind, text) in tokens {
        if text.is_empty() {
            out.push(text);
            continue;
        }
        let last = last_char(&text);
        if kind == TokenKind::Literal {
            previous_last = last;
            out.push(text);
            continue;
        }
        let mut text = process_escapes(&text);
        if actions.convert_quot {
            text = text.replace("&quot;", "\"");
        }
        match actions.dashes {
            1 => text = educate_dashes(&text),
            2 => text = educate_dashes_old_school(&text),
            3 => text = educate_dashes_old_school_inverted(&text),
            _ => {}
        }
        if actions.ellipses {
            text = educate_ellipses(&text);
        }
        if actions.backticks != 0 {
            text = educate_backticks(&text, language);
        }
        if actions.backticks == 2 {
            text = educate_single_backticks(&text, language);
        }
        if actions.quotes {
            // "Replace plain quotes in context to prevent conversion to
            // 2-character sequence in French." (`:661-665`)
            let context = previous_last.replace(['"', '\''], ";");
            let educated = educate_quotes(&format!("{context}{text}"), language);
            let mut rest = educated.chars();
            rest.next();
            text = rest.as_str().to_string();
        }
        if actions.stupefy {
            text = stupefy_entities(&text, language);
        }
        previous_last = last;
        out.push(restore_escapes(&text));
    }
    out
}

/// `text[-1:]`.
fn last_char(text: &str) -> String {
    text.chars().last().map(String::from).unwrap_or_default()
}

/// `processEscapes`' table (`smartquotes.py:850-880`).
const ESCAPES: [(&str, &str); 6] = [
    ("\\\\", "&#92;"),
    ("\\\"", "&#34;"),
    ("\\'", "&#39;"),
    ("\\.", "&#46;"),
    ("\\-", "&#45;"),
    ("\\`", "&#96;"),
];

/// `processEscapes(text)`: each backslash escape replaced by its entity.
fn process_escapes(text: &str) -> String {
    ESCAPES
        .iter()
        .fold(text.to_string(), |text, (escape, entity)| {
            text.replace(escape, entity)
        })
}

/// `processEscapes(text, restore=True)`: each entity back to the character
/// alone — a literal `&#34;` in the text included.
fn restore_escapes(text: &str) -> String {
    ESCAPES
        .iter()
        .fold(text.to_string(), |text, (escape, entity)| {
            text.replace(entity, &escape[1..])
        })
}

/// `educateDashes` (`smartquotes.py:769-779`): `---` an en dash, `--` an
/// em dash ("yes, backwards").
fn educate_dashes(text: &str) -> String {
    text.replace("---", tables::ENDASH)
        .replace("--", tables::EMDASH)
}

/// `educateDashesOldSchool` (`:781-792`): `---` an em dash, `--` an en dash.
fn educate_dashes_old_school(text: &str) -> String {
    text.replace("---", tables::EMDASH)
        .replace("--", tables::ENDASH)
}

/// `educateDashesOldSchoolInverted` (`:794-811`): as [`educate_dashes`].
fn educate_dashes_old_school_inverted(text: &str) -> String {
    text.replace("---", tables::ENDASH)
        .replace("--", tables::EMDASH)
}

/// `educateEllipses` (`:813-826`).
fn educate_ellipses(text: &str) -> String {
    text.replace("...", tables::ELLIPSIS)
        .replace(". . .", tables::ELLIPSIS)
}

/// `educateBackticks` (`:738-751`): ``` `` ``` and `''` the primary quotes.
fn educate_backticks(text: &str, language: &str) -> String {
    let [opquote, cpquote, _, _] = smartchars(language);
    text.replace("``", opquote).replace("''", cpquote)
}

/// `educateSingleBackticks` (`:753-767`): every `` ` `` and `'` a
/// secondary quote.
fn educate_single_backticks(text: &str, language: &str) -> String {
    let [_, _, osquote, csquote] = smartchars(language);
    text.replace('`', osquote).replace('\'', csquote)
}

/// `stupefyEntities` (`:828-848`): the smart characters back to ASCII.
fn stupefy_entities(text: &str, language: &str) -> String {
    let [opquote, cpquote, osquote, csquote] = smartchars(language);
    text.replace(tables::ENDASH, "-")
        .replace(tables::EMDASH, "--")
        .replace(osquote, "'")
        .replace(csquote, "'")
        .replace(opquote, "\"")
        .replace(cpquote, "\"")
        .replace(tables::ELLIPSIS, "...")
}

/// Python `re`'s `\w` on a `str` pattern.
fn is_word(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    let code = u32::from(c);
    let run = tables::WORD_RUNS.partition_point(|&(first, _)| first <= code);
    run > 0 && code <= tables::WORD_RUNS[run - 1].1
}

/// Python `re`'s `\s` on a `str` pattern.
fn is_space(c: char) -> bool {
    crate::utils::py_isspace(c)
}

/// Python `re`'s `\d` on a `str` pattern.
fn is_digit(c: char) -> bool {
    crate::utils::py_decimal(c).is_some()
}

/// `_CH_CLASSES['punct']`.
fn is_punct(c: char) -> bool {
    tables::PUNCT.binary_search(&c).is_ok()
}

/// `_CH_CLASSES['open']`.
fn is_open(c: char) -> bool {
    tables::OPEN.contains(&c)
}

/// `_CH_CLASSES['dash']`.
fn is_dash(c: char) -> bool {
    tables::DASH.contains(&c)
}

/// `_CH_CLASSES['sep']`: `\s`, ZWSP, ZWNJ.
fn is_sep(c: char) -> bool {
    is_space(c) || tables::SEP_EXTRA.contains(&c)
}

/// `re.sub` of a pattern matching exactly `len` characters: scan `text`
/// left to right, and where `matches(text, at)` holds, write
/// `replace(text, at)` for those characters and resume after them.
fn sub(
    text: &[char],
    len: usize,
    matches: impl Fn(&[char], usize) -> bool,
    replace: impl Fn(&[char], usize, &mut Vec<char>),
) -> Vec<char> {
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        if at + len <= text.len() && matches(text, at) {
            replace(text, at, &mut out);
            at += len;
        } else {
            out.push(text[at]);
            at += 1;
        }
    }
    out
}

/// A replacement writing `quote` for the whole match.
fn write(quote: &str) -> impl Fn(&[char], usize, &mut Vec<char>) + '_ {
    move |_, _, out| out.extend(quote.chars())
}

/// A replacement writing the match's first character (`\1`), then `quote`.
fn keep_first_then(quote: &str) -> impl Fn(&[char], usize, &mut Vec<char>) + '_ {
    move |text, at, out| {
        out.push(text[at]);
        out.extend(quote.chars());
    }
}

/// `\B` at `at` (Python 3.12 `SRE_AT_UNI_NON_BOUNDARY`): never in an empty
/// string, else the characters either side equally word or non-word (a
/// missing one non-word). Only START_SINGLE/START_DOUBLE use it, in the
/// reading [`educate_quotes`] gives them — not upstream's literal pattern.
fn non_boundary(text: &[char], at: usize) -> bool {
    if text.is_empty() {
        return false;
    }
    let before = at > 0 && text.get(at - 1).is_some_and(|&c| is_word(c));
    let after = text.get(at).is_some_and(|&c| is_word(c));
    before == after
}

/// `educateQuotes(text, language)` (`smartquotes.py:678-735`) over the
/// regular expressions of `:508-555`, each a scanner in its order.
pub(crate) fn educate_quotes(text: &str, language: &str) -> String {
    if !text.contains(['-', '"', '\'']) {
        return text.to_string();
    }
    let [opquote, cpquote, osquote, csquote] = smartchars(language);
    let apostrophe = tables::APOSTROPHE;
    let at = |text: &[char], index: usize| text.get(index).copied();
    let mut s: Vec<char> = text.chars().collect();

    // START_SINGLE / START_DOUBLE (`smartquotes.py:516-517`): upstream's
    // pattern is `r"^'(?=%s\\B)" % punct` — punctuation, then a LITERAL
    // backslash and `B` (in the raw string, `\\` is `re`'s escaped
    // backslash), not the non-word-boundary `\B`.
    // Read here as `\B`, a non-word-boundary, with the same outcome: a
    // quote at position 0 that either reading closes is closed anyway by
    // CLOSING_SECONDARY/CLOSING_PRIMARY (nothing precedes it), and the only
    // rules that would treat it otherwise — ADJACENT_* (a word character
    // two along) and DECADE (a digit next) — never fire where this reading
    // does (punctuation next, no boundary after it). Checked against
    // docutils itself: every quote-led string of up to 5 characters over
    // `'"`.-_ ax1\B–(\u{a0};,s8` in 6 languages, 1,650,732 evaluations,
    // no difference. Inside `educate_tokens` position 0 is the context
    // character, never a quote, so neither reading ever fires there.
    for (quote, replacement) in [('\'', csquote), ('"', cpquote)] {
        s = sub(
            &s,
            1,
            |s, i| i == 0 && s[i] == quote && at(s, 1).is_some_and(is_punct) && non_boundary(s, 2),
            write(replacement),
        );
    }
    // ADJACENT_1 / ADJACENT_2: `"'(?=\w)`, `'"(?=\w)`.
    let primary_then_secondary = format!("{opquote}{osquote}");
    s = sub(
        &s,
        2,
        |s, i| s[i] == '"' && s[i + 1] == '\'' && at(s, i + 2).is_some_and(is_word),
        write(&primary_then_secondary),
    );
    let secondary_then_primary = format!("{osquote}{opquote}");
    s = sub(
        &s,
        2,
        |s, i| s[i] == '\'' && s[i + 1] == '"' && at(s, i + 2).is_some_and(is_word),
        write(&secondary_then_primary),
    );
    // OPEN_SINGLE / OPEN_DOUBLE: `(open|dash)'(?=P? )` — closing.
    for (quote, replacement) in [('\'', csquote), ('"', cpquote)] {
        s = sub(
            &s,
            2,
            |s, i| {
                (is_open(s[i]) || is_dash(s[i]))
                    && s[i + 1] == quote
                    && (at(s, i + 2) == Some(' ')
                        || (at(s, i + 2).is_some_and(is_punct) && at(s, i + 3) == Some(' ')))
            },
            keep_first_then(replacement),
        );
    }
    // DECADE, English only: `'(?=\d{2}s)` — the '80s.
    if language.starts_with("en") {
        s = sub(
            &s,
            1,
            |s, i| {
                s[i] == '\''
                    && at(s, i + 1).is_some_and(is_digit)
                    && at(s, i + 2).is_some_and(is_digit)
                    && at(s, i + 3) == Some('s')
            },
            write(apostrophe),
        );
    }
    // OPENING_SECONDARY: `(sep|open|dash)'(?=\w|P)`.
    s = sub(&s, 2, |s, i| opens(s, i, '\''), keep_first_then(osquote));
    // APOSTROPHE: `(?<=(\w|\d))'(?=\w)`, where the closing secondary quote
    // is not the apostrophe already.
    if csquote != apostrophe {
        s = sub(
            &s,
            1,
            |s, i| {
                s[i] == '\''
                    && i > 0
                    && (is_word(s[i - 1]) || is_digit(s[i - 1]))
                    && at(s, i + 1).is_some_and(is_word)
            },
            write(apostrophe),
        );
    }
    // CLOSING_SECONDARY: `(?<!\s)'`; every other `'` opens.
    s = sub(
        &s,
        1,
        |s, i| s[i] == '\'' && (i == 0 || !is_space(s[i - 1])),
        write(csquote),
    );
    s = sub(&s, 1, |s, i| s[i] == '\'', write(osquote));
    // OPENING_PRIMARY: `(sep|open|dash)"(?=\w|P)`.
    s = sub(&s, 2, |s, i| opens(s, i, '"'), keep_first_then(opquote));
    // CLOSING_PRIMARY: `(?<!\s)"|"(?=\s)`; every other `"` opens.
    s = sub(
        &s,
        1,
        |s, i| s[i] == '"' && (i == 0 || !is_space(s[i - 1]) || at(s, i + 1).is_some_and(is_space)),
        write(cpquote),
    );
    s = sub(&s, 1, |s, i| s[i] == '"', write(opquote));
    s.into_iter().collect()
}

/// `(sep|open|dash)<quote>(?=\w|P)` at `i` — OPENING_SECONDARY and
/// OPENING_PRIMARY.
fn opens(s: &[char], i: usize, quote: char) -> bool {
    (is_sep(s[i]) || is_open(s[i]) || is_dash(s[i]))
        && s[i + 1] == quote
        && s.get(i + 2).is_some_and(|&c| is_word(c) || is_punct(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rst::ParseOptions;

    /// `normalize_language_tag` as docutils writes it (`__init__.py:
    /// 741-765`): every combination, exponential — the reference
    /// [`quotes_tag`] is checked against on small tags.
    fn normalize_language_tag(tag: &str) -> Vec<String> {
        let (base, subtags) = split_language_tag(tag);
        let mut list = Vec::new();
        for count in (1..=subtags.len()).rev() {
            for combination in index_combinations(subtags.len(), count) {
                let mut parts = vec![base.clone()];
                parts.extend(combination.iter().map(|&index| subtags[index].clone()));
                list.push(parts.join("-"));
            }
        }
        list.push(base);
        list
    }

    fn plain(text: &str) -> (TokenKind, String) {
        (TokenKind::Plain, text.to_string())
    }

    fn educate(text: &str) -> String {
        educate_tokens(vec![plain(text)], "qDe", "en").remove(0)
    }

    /// The generated classes are Python's: `\s` is `py_isspace`, and `\d`
    /// has as many members as `py_decimal` knows; `PUNCT` is sorted for
    /// its binary search.
    #[test]
    fn the_character_classes_are_pythons() {
        let spaces: Vec<u32> = (0..=0x10FFFF)
            .filter_map(char::from_u32)
            .filter(|&c| is_space(c))
            .map(u32::from)
            .collect();
        assert_eq!(spaces, tables::SPACE);
        let digits = (0..=0x10FFFF)
            .filter_map(char::from_u32)
            .filter(|&c| is_digit(c))
            .count();
        assert_eq!(digits, tables::DIGIT_COUNT);
        assert!(tables::PUNCT.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(tables::QUOTES.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert!(is_punct(' ') && !is_punct('&'));
        assert!(is_word('é') && is_word('_') && is_word('²') && !is_word('\u{345}'));
    }

    /// The probed outputs of research §5.3 (default `qDe`, `en`).
    #[test]
    fn educates_like_docutils() {
        assert_eq!(
            educate("\"Quoted\" Title's -- here"),
            "\u{201c}Quoted\u{201d} Title\u{2019}s \u{2013} here"
        );
        assert_eq!(
            educate(
                "He said \"hello\" and 'bye'. It's the '80s -- or 1990--2000 --- maybe... ok. . . done."
            ),
            "He said \u{201c}hello\u{201d} and \u{2018}bye\u{2019}. It\u{2019}s the \u{2019}80s \u{2013} or \
             1990\u{2013}2000 \u{2014} maybe\u{2026} ok\u{2026} done."
        );
        assert_eq!(
            educate(
                "He said \"she said 'hi'\" -- ok. x--y and 5'10\" and rock 'n' roll. \"Hello,\" she said. 'Twas."
            ),
            "He said \u{201c}she said \u{2018}hi\u{2019}\u{201d} \u{2013} ok. x\u{2013}y and \
             5\u{2019}10\u{201d} and rock \u{2018}n\u{2019} roll. \u{201c}Hello,\u{201d} she said. \u{2018}Twas."
        );
        assert_eq!(
            educate("Escaped \\\"quote\\\" and \\-- and \\... and \\'x\\'."),
            "Escaped \"quote\" and -- and ... and 'x'."
        );
    }

    /// A literal token is untouched but is context: a quote after literal
    /// text closes; `'` after it is an apostrophe.
    #[test]
    fn a_literal_token_is_context() {
        let tokens = vec![
            plain("'Start' and \""),
            (TokenKind::Literal, "emph".to_string()),
            plain("\" end. "),
            (TokenKind::Literal, "code".to_string()),
            plain("'s apostrophe."),
        ];
        assert_eq!(
            educate_tokens(tokens, "qDe", "en"),
            [
                "\u{2018}Start\u{2019} and \u{201c}",
                "emph",
                "\u{201d} end. ",
                "code",
                "\u{2019}s apostrophe."
            ]
        );
    }

    /// French quotes carry a no-break space; German open low; an unknown
    /// language keeps ASCII quotes but still makes apostrophes; `q` alone
    /// leaves dashes and dots; `-1` stupefies.
    #[test]
    fn languages_and_actions() {
        let one = |text: &str, attr: &str, language: &str| {
            educate_tokens(vec![plain(text)], attr, language).remove(0)
        };
        assert_eq!(
            one("\"Bonjour\" 'x'", "qDe", "fr"),
            "\u{ab}\u{a0}Bonjour\u{a0}\u{bb} \u{201c}x\u{201d}"
        );
        assert_eq!(
            one("\"Top\" -- 'x'", "qDe", "de"),
            "\u{201e}Top\u{201c} \u{2013} \u{201a}x\u{2018}"
        );
        assert_eq!(
            one("\"Quoted\" it's -- x...", "qDe", ""),
            "\"Quoted\" it\u{2019}s \u{2013} x\u{2026}"
        );
        assert_eq!(one("\"x\" -- y...", "q", "en"), "\u{201c}x\u{201d} -- y...");
        assert_eq!(one("a -- b --- c", "qde", "en"), "a \u{2014} b \u{2013} c");
        assert_eq!(
            one("\u{201c}x\u{201d} \u{2014} y\u{2026}", "-1", "en"),
            "\"x\" -- y..."
        );
    }

    /// `normalize_language_tag`'s docstring examples, and [`quotes_tag`]
    /// naming the first tag of that list the table has, for every tag
    /// built from a few subtags the table knows and some it does not.
    #[test]
    fn the_first_quote_tag_is_normalize_language_tags() {
        assert_eq!(
            normalize_language_tag("de_AT-1901"),
            ["de-at-1901", "de-at", "de-1901", "de"]
        );
        assert_eq!(
            normalize_language_tag("de-CH-x_altquot"),
            ["de-ch-x-altquot", "de-ch", "de-x-altquot", "de"]
        );
        let pieces = ["ch", "x", "altquot", "uk", "zz", "x-altquot", "CH", ""];
        let bases = ["de", "en", "fr", "zh", "xx", ""];
        let mut checked = 0;
        for base in bases {
            for a in pieces {
                for b in pieces {
                    for c in pieces {
                        let tag = format!("{base}_{a}-{b}_{c}");
                        let expected = normalize_language_tag(&tag)
                            .into_iter()
                            .find(|tag| quotes_entry(tag).is_some());
                        assert_eq!(quotes_tag(&tag).map(str::to_string), expected, "{tag:?}");
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, 6 * 8 * 8 * 8);
        assert_eq!(quotes_tag("de-xx"), Some("de"));
        assert_eq!(quotes_tag("en-UK-x-altquot"), Some("en-uk-x-altquot"));
        assert_eq!(quotes_tag("yy-zz"), None);
    }

    /// A tag of thousands of subtags — an exponential list in docutils —
    /// is answered at once.
    #[test]
    fn a_long_language_tag_terminates() {
        let tag = format!("fr{}", "-zz".repeat(5000) + "-ch-x-altquot");
        assert_eq!(quotes_tag(&tag), Some("fr-ch-x-altquot"));
        let none = format!("qq{}", "-zz".repeat(5000));
        assert_eq!(quotes_tag(&none), None);
    }

    /// No input panics or hangs the educator: every action over strings of
    /// the active characters, escapes, nulls, classes and multi-character
    /// French quotes.
    #[test]
    fn educating_any_text_terminates() {
        let alphabet: Vec<char> = "\"'`-.\\ \u{0}a1_é\u{a0}\u{2013}([{&#;:s\u{200b}\u{3000}\u{1f}"
            .chars()
            .collect();
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..2000 {
            let len = (next() % 24) as usize;
            let text: String = (0..len)
                .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                .collect();
            for attr in ["qDe", "1", "2", "3", "-1", "qBbdiew", "0"] {
                for language in ["en", "fr", "de", "", "fr-ch-x-altquot"] {
                    let tokens = vec![
                        plain(&text),
                        (TokenKind::Literal, text.clone()),
                        plain(&text),
                    ];
                    assert_eq!(educate_tokens(tokens, attr, language).len(), 3);
                    educate_quotes(&text, language);
                }
            }
        }
    }

    fn sq_opts() -> ParseOptions {
        ParseOptions {
            source_path: "<snippet>".to_string(),
            sphinx: true,
            ..Default::default()
        }
    }

    /// The educated text keeps the nulls docutils keeps: an escaped quote
    /// stays plain and keeps its escape, so a second educating (a nested
    /// unit's) still sees it; a literal token's node loses them, as
    /// `nodes.Text(astext())` does.
    #[test]
    fn educated_text_keeps_its_escapes() {
        let (tree, records) = crate::transforms::parse_and_transform(
            "A \\\"b\\\" \"c\".\n",
            &sq_opts(),
            &TransformConfig::default(),
        );
        assert_eq!(records, []);
        let text = &tree.root.children[0].children[0];
        assert_eq!(text.text.as_deref(), Some("A \"b\" \u{201c}c\u{201d}."));
        assert_eq!(
            text.null_escaped().as_deref(),
            Some("A \u{0}\"b\u{0}\" \u{201c}c\u{201d}.")
        );
    }

    /// The unsupported-language WARNING is located at the unit
    /// (`base_node=node`), wherever the class comes from — here a list
    /// item's paragraph inside a classed container — once per language per
    /// document; a `language-` class the directive normalizes to `language`
    /// names none, and the document's applies. Probed under Sphinx 9.1: one
    /// warning, at line 5; `Empty “e”.` educated.
    #[test]
    fn the_unsupported_language_warning_is_at_the_unit() {
        let (tree, records) = crate::transforms::parse_and_transform(
            "Para \"p\".\n\n.. container:: language-qq\n\n   - item \"i\"\n\n     | line \"l\"\n\n\
             .. rst-class:: language-qq\n\nAgain \"a\".\n\n.. container:: language-\n\n   Empty \"e\".\n",
            &sq_opts(),
            &TransformConfig::default(),
        );
        let records: Vec<(u8, Option<u32>, &str)> = records
            .iter()
            .map(|record| (record.level, record.line, record.text.as_str()))
            .collect();
        assert_eq!(
            records,
            [(
                messages::WARNING,
                Some(5),
                "No smart quotes defined for language \"qq\"."
            )]
        );
        let texts: Vec<String> = tree.root.children.iter().map(Node::astext).collect();
        assert_eq!(
            texts,
            [
                "Para \u{201c}p\u{201d}.",
                "item \"i\"line \"l\"",
                "Again \"a\".",
                "Empty \u{201c}e\u{201d}."
            ]
        );
    }

    /// `parent.replace(txtnode, Text(newtext))` replaces the first child
    /// equal to the node (`list.index`; a docutils `Text` is a `str`):
    /// probed under Sphinx 9.1, `&#34;x *a*"x *b*"x *c*"x` gives `”x a ”x
    /// b "x c ”x` — the first text came back as `"x ` (its entity
    /// restored), which the second text's educated `”x ` then replaced,
    /// the third took the second's slot and kept its own straight one. The
    /// compared `str()` keeps its nulls: an escaped `\"x ` is no `"x `
    /// (`"x a ”x b`).
    #[test]
    fn the_first_equal_text_is_replaced() {
        let paragraph = |source: &str| {
            let (tree, _) = crate::transforms::parse_and_transform(
                source,
                &sq_opts(),
                &TransformConfig::default(),
            );
            tree.root.children[0]
                .children
                .iter()
                .map(Node::astext)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            paragraph("&#34;x *a*\"x *b*\"x *c*\"x\n"),
            [
                "\u{201d}x ",
                "a",
                "\u{201d}x ",
                "b",
                "\"x ",
                "c",
                "\u{201d}x"
            ]
        );
        assert_eq!(
            paragraph("\\\"x *a*\"x *b*\n"),
            ["\"x ", "a", "\u{201d}x ", "b"]
        );
    }

    /// `smartquotes_excludes` and the master switch: a builder or language
    /// it names, a language with no quotes, or `smartquotes = False` leave
    /// the tree alone.
    #[test]
    fn availability_follows_the_configuration() {
        use crate::config::{ExcludeList, SmartquotesExcludes};
        let educated = |config: &TransformConfig| {
            let (tree, _) =
                crate::transforms::parse_and_transform("\"x\" -- y\n", &sq_opts(), config);
            tree.root.children[0].astext()
        };
        assert_eq!(
            educated(&TransformConfig::default()),
            "\u{201c}x\u{201d} \u{2013} y"
        );
        for config in [
            TransformConfig {
                smartquotes: false,
                ..TransformConfig::default()
            },
            TransformConfig {
                builder: "text".to_string(),
                ..TransformConfig::default()
            },
            TransformConfig {
                language: "zh_CN".to_string(),
                ..TransformConfig::default()
            },
            TransformConfig {
                language: "xx".to_string(),
                ..TransformConfig::default()
            },
            TransformConfig {
                smartquotes_excludes: SmartquotesExcludes {
                    languages: ExcludeList::Text("xenx".to_string()),
                    builders: ExcludeList::default(),
                },
                ..TransformConfig::default()
            },
        ] {
            assert_eq!(educated(&config), "\"x\" -- y", "{config:?}");
        }
    }
}
