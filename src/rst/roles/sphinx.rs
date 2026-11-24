//! Sphinx's role functions (`SP/sphinx/roles.py`,
//! `SP/sphinx/domains/index.py:97-117`) and the `XRefRole` every
//! cross-reference role is built on, with the domain variants this crate
//! models ([RC §3]).
//!
//! A role function here gets the text as Sphinx's role gets it: the name
//! `SphinxRole.__call__` settled on (the name written, lowercased, or the
//! document's default role; [`RoleCall::name`]), the raw text with its
//! backslashes restored, and the interpreted text with docutils' null
//! escapes, which `SphinxRole` unescapes and `ReferenceRole` splits on
//! ([`ReferenceParts`]).

use crate::doctree::{kinds, unescape, AttrValue, Node};
use crate::pyformat::{self, FormatError, Value};
use crate::rst::inline::RefContext;
use crate::rst::LogLocation;
use crate::transforms::state::DocumentState;

use super::reference::{explicit_title, ReferenceParts};
use super::{DomainId, RoleCall, SphinxCrash};

/// `nodes.literal`, `nodes.inline`, … over `rawtext` with these classes:
/// the shape most of Sphinx's roles build.
fn element(kind: &'static str, call: &RoleCall<'_>, classes: Vec<String>) -> Node {
    let mut node = Node::elem(kind, call.span);
    node.set_rawsource(call.rawtext);
    node.attrs.classes = classes;
    node
}

/// `nodes.Text(text)`, unless `text` is empty: a `TextElement` built with
/// empty text gets no child (`nodes.TextElement.__init__`).
fn push_text(node: &mut Node, text: &str, call: &RoleCall<'_>) {
    if !text.is_empty() {
        node.children.push(Node::text_node(text, call.span));
    }
}

/// The node class of one of Sphinx's `generic_docroles`
/// (`SP/sphinx/roles.py:28-37`).
fn generic_docrole_kind(registered: &str) -> &'static str {
    match registered {
        "command" | "makevar" | "program" => "literal_strong",
        "dfn" => kinds::EMPHASIS,
        "mailheader" | "mimetype" | "newsgroup" => "literal_emphasis",
        _ => kinds::LITERAL,
    }
}

/// One of Sphinx's `generic_docroles`: `CustomRole(name,
/// GenericRole(name, nodeclass), {'classes': [name]})`
/// (`SP/sphinx/roles.py:633-636`), so the node's classes are the
/// registered name's unless the call passes its own (a custom role over
/// it replaces them). The text keeps docutils' null escapes, as
/// `GenericRole` gives it ([RC §3.1]).
pub(crate) fn sphinx_generic(call: &RoleCall<'_>, registered: &'static str) -> Node {
    let classes = call
        .options
        .classes
        .clone()
        .unwrap_or_else(|| vec![registered.to_string()]);
    let mut node = element(generic_docrole_kind(registered), call, classes);
    node.children
        .push(Node::text_node_escaped(call.escaped, call.span));
    node
}

/// `Keyboard._pattern.split(text)`: `re.split(r'(?<=.)([\-+^]| +)(?=.)',
/// text)`, keys and the separators between them. `.` is any character
/// but a newline, so a separator next to one does not split
/// (`SP/sphinx/roles.py:486`).
fn kbd_split(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let any = |at: usize| at < n && chars[at] != '\n';
    let mut parts = Vec::new();
    let mut last = 0;
    let mut i = 0;
    while i < n {
        if i > 0 && chars[i - 1] != '\n' {
            // `[\-+^]` with a character after it, else ` +`: the longest
            // run of spaces with a character after it.
            let separator = if matches!(chars[i], '-' | '+' | '^') && any(i + 1) {
                Some(1)
            } else if chars[i] == ' ' {
                let run = chars[i..].iter().take_while(|c| **c == ' ').count();
                (1..=run).rev().find(|len| any(i + len))
            } else {
                None
            };
            if let Some(len) = separator {
                parts.push(chars[last..i].iter().collect());
                parts.push(chars[i..i + len].iter().collect());
                i += len;
                last = i;
                continue;
            }
        }
        i += 1;
    }
    parts.push(chars[last..].iter().collect());
    parts
}

/// `Keyboard._is_multi_word_key` (`:515-528`): the first three parts are
/// a key of two words with spaces between them.
fn is_multi_word_key(parts: &[String]) -> bool {
    if parts.len() <= 2 || parts[1].is_empty() || !parts[1].chars().all(crate::utils::py_isspace) {
        return false;
    }
    let name = (parts[0].to_lowercase(), parts[2].to_lowercase());
    matches!(
        (name.0.as_str(), name.1.as_str()),
        ("back", "space")
            | ("caps", "lock")
            | ("num", "lock")
            | ("page", "down")
            | ("page", "up")
            | ("scroll", "lock")
            | ("sys", "rq")
    )
}

/// Sphinx's `Keyboard` (`SP/sphinx/roles.py:478-528`): one `literal`
/// classed `kbd` (and the call's classes) per key, the separators between
/// them as text; the whole text one key when it does not split or starts
/// with a key of two words ([RC §3.2]).
pub(crate) fn kbd(call: &RoleCall<'_>) -> Vec<Node> {
    let text = unescape(call.escaped, false);
    let mut classes = vec!["kbd".to_string()];
    classes.extend(call.options.classes.iter().flatten().cloned());
    let literal = |rawsource: &str, key: &str| {
        let mut node = Node::elem(kinds::LITERAL, call.span);
        node.set_rawsource(rawsource);
        node.attrs.classes = classes.clone();
        push_text(&mut node, key, call);
        node
    };
    let mut parts = kbd_split(&text);
    if parts.len() == 1 || is_multi_word_key(&parts) {
        return vec![literal(call.rawtext, &text)];
    }
    let mut compound = Vec::new();
    while !parts.is_empty() {
        let key = if is_multi_word_key(&parts) {
            parts.drain(..3).collect::<String>()
        } else {
            parts.remove(0)
        };
        compound.push(literal(&key, &key));
        if parts.is_empty() {
            break;
        }
        compound.push(Node::text_node(parts.remove(0), call.span));
    }
    compound
}

/// `GUILabel.amp_re.split(text)`: `(?<!&)&(?![&\s])`, an `&` with no `&`
/// before it and neither an `&` nor whitespace after it, the end of the
/// text included (`SP/sphinx/roles.py:388`).
fn accelerator_spans(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut last = 0;
    for i in 0..chars.len() {
        let after_ok = chars
            .get(i + 1)
            .is_none_or(|c| *c != '&' && !crate::utils::py_isspace(*c));
        if chars[i] == '&' && (i == 0 || chars[i - 1] != '&') && after_ok {
            spans.push(chars[last..i].iter().collect());
            last = i + 1;
        }
    }
    spans.push(chars[last..].iter().collect());
    spans
}

/// Sphinx's `GUILabel` and `MenuSelection` (`SP/sphinx/roles.py:387-410`):
/// an `inline` classed with the role's name whose `rawtext` is an
/// attribute, the character after each accelerator `&` in an `inline`
/// classed `accelerator`, `&&` an `&` within those spans only, and for a
/// menu selection every `-->` a `‣` first ([RC §3.3-3.4]). An `&` that
/// ends the text leaves an empty span, whose first character Sphinx takes
/// anyway: `IndexError`, a crash (decision 13).
pub(crate) fn guilabel(
    call: &RoleCall<'_>,
    menuselection: bool,
    location: LogLocation,
) -> Result<Node, SphinxCrash> {
    let mut text = unescape(call.escaped, false);
    if menuselection {
        text = text.replace("-->", "\u{2023}");
    }
    let mut node = Node::elem("inline", call.span);
    node.attrs.classes = vec![call.name.to_string()];
    node.set("rawtext", AttrValue::Str(call.rawtext.to_string()));
    let mut spans = accelerator_spans(&text).into_iter();
    node.children
        .push(Node::text_node(spans.next().unwrap_or_default(), call.span));
    for span in spans {
        let span = span.replace("&&", "&");
        let mut letters = span.chars();
        let Some(letter) = letters.next() else {
            return Err(SphinxCrash::python(
                "IndexError",
                "string index out of range".to_string(),
                location,
            ));
        };
        let mut accelerator = Node::elem("inline", call.span);
        accelerator.attrs.classes = vec!["accelerator".to_string()];
        accelerator
            .children
            .push(Node::text_node(letter.to_string(), call.span));
        node.children.push(accelerator);
        node.children
            .push(Node::text_node(letters.as_str(), call.span));
    }
    Ok(node)
}

/// `EmphasizedLiteral.parens_re.split(text)`, `(\\\\|\\{|\\}|{|})`: the
/// tokens and the text between them, empty strings left out.
fn brace_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut plain = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let token = match (c, chars.peek()) {
            ('\\', Some(next @ ('\\' | '{' | '}'))) => {
                let token = format!("\\{next}");
                chars.next();
                token
            }
            ('{' | '}', _) => c.to_string(),
            _ => {
                plain.push(c);
                continue;
            }
        };
        if !plain.is_empty() {
            tokens.push(std::mem::take(&mut plain));
        }
        tokens.push(token);
    }
    if !plain.is_empty() {
        tokens.push(plain);
    }
    tokens
}

/// Sphinx's `EmphasizedLiteral` (`SP/sphinx/roles.py:413-460`), the `samp`
/// and `file` roles: a `literal` with `classes` and `role` the role's
/// name, `{word}` an `emphasis`, through `parse`'s three-slot stack over
/// the unescaped text, ported as written ([RC §3.5]).
pub(crate) fn emphasized_literal(call: &RoleCall<'_>) -> Node {
    let mut node = element(kinds::LITERAL, call, vec![call.name.to_string()]);
    node.set("role", AttrValue::Str(call.name.to_lowercase()));
    let mut stack = vec![String::new()];
    for part in brace_tokens(&unescape(call.escaped, false)) {
        match part.as_str() {
            "\\\\" => stack.last_mut().expect("never empty").push('\\'),
            "{" => {
                if stack.len() >= 2 && stack[stack.len() - 2] == "{" {
                    stack.last_mut().expect("never empty").push('{');
                } else {
                    stack.extend(["{".to_string(), String::new()]);
                }
            }
            "}" => {
                if stack.len() == 3 && stack[1] == "{" && !stack[2].is_empty() {
                    if !stack[0].is_empty() {
                        node.children
                            .push(Node::text_node(stack[0].clone(), call.span));
                    }
                    let mut emphasis = Node::elem(kinds::EMPHASIS, call.span);
                    emphasis.set_rawsource(&stack[2]);
                    emphasis
                        .children
                        .push(Node::text_node(stack[2].clone(), call.span));
                    node.children.push(emphasis);
                    stack = vec![String::new()];
                } else {
                    stack.push("}".to_string());
                    stack = vec![stack.concat()];
                }
            }
            "\\{" => stack.last_mut().expect("never empty").push('{'),
            "\\}" => stack.last_mut().expect("never empty").push('}'),
            other => stack.last_mut().expect("never empty").push_str(other),
        }
    }
    let rest = stack.concat();
    if !rest.is_empty() {
        node.children.push(Node::text_node(rest, call.span));
    }
    node
}

/// Sphinx's `Abbreviation` (`SP/sphinx/roles.py:463-475`):
/// `abbr_re.search(text)`, `\((.*)\)$` with DOTALL, from the first `(` to
/// a final `)`: the stripped text before it and the `explanation` inside;
/// the call's options are copied onto the node ([RC §3.6]).
pub(crate) fn abbr(call: &RoleCall<'_>) -> Node {
    let text = unescape(call.escaped, false);
    let classes = call.options.classes.clone().unwrap_or_default();
    let mut node = element(kinds::ABBREVIATION, call, classes);
    let explained = text.strip_suffix(')').and_then(|head| head.split_once('('));
    match explained {
        Some((before, explanation)) => {
            node.set("explanation", AttrValue::Str(explanation.to_string()));
            push_text(
                &mut node,
                before.trim_matches(crate::utils::py_isspace),
                call,
            );
        }
        None => push_text(&mut node, &text, call),
    }
    node
}

/// `Manpage._manpage_re.match(manpage)`,
/// `^(?P<path>(?P<page>.+)[(.](?P<section>[1-9]\w*)?\)?)$`: the greedy
/// page and the section after its `(` or `.`, `None` when the match has
/// none; no match is `{'path': t, 'page': t, 'section': ''}`.
fn manpage_info(manpage: &str) -> (String, String, Option<String>) {
    let chars: Vec<char> = manpage.chars().collect();
    let n = chars.len();
    let section_of = |rest: &[char]| -> Option<Option<String>> {
        let body = rest.strip_suffix(&[')']).unwrap_or(rest);
        match body.split_first() {
            None => Some(None),
            Some((first, tail)) => (('1'..='9').contains(first)
                && tail.iter().all(|c| crate::rst::inline::is_word_char(*c)))
            .then(|| Some(body.iter().collect())),
        }
    };
    if !chars.contains(&'\n') {
        for split in (1..n).rev() {
            if !matches!(chars[split], '(' | '.') {
                continue;
            }
            if let Some(section) = section_of(&chars[split + 1..]) {
                return (
                    manpage.to_string(),
                    chars[..split].iter().collect(),
                    section,
                );
            }
        }
    }
    (
        manpage.to_string(),
        manpage.to_string(),
        Some(String::new()),
    )
}

/// `manpages_url.format_map(info)`, Python's own `str.format_map`
/// ([`pyformat::format_map`]) over the page's `path`, `page` and
/// `section`, which is `None` when the target names no section. Whatever
/// it raises, a `KeyError` for a field that names no key among them, a
/// `ValueError` for a positional field, a format it cannot read or a spec
/// it does not know, a `TypeError` for a spec on `None`, `Manpage.run`
/// does not catch: Sphinx 9.1.0 crashes at parse time (decision 13), where
/// the role is. A field that looks up an attribute its value has
/// (`{page.upper}`) is not modelled (T19b's divergence list): Sphinx
/// formats the attribute's text, a bound method's, which names its address
/// in memory, and reads on, so a later field still raises what it raises
/// ([`FormatError::NotModelled`]); where nothing is raised, the reference
/// keeps `manpages_url` as written.
fn format_manpages_url(
    url: &str,
    info: &(String, String, Option<String>),
    location: LogLocation,
) -> Result<String, SphinxCrash> {
    let section = info.2.as_deref().map_or(Value::None, Value::Str);
    let mapping = [
        ("path", Value::Str(&info.0)),
        ("page", Value::Str(&info.1)),
        ("section", section),
    ];
    match pyformat::format_map(url, &mapping) {
        Ok(uri) => Ok(uri),
        Err(FormatError::NotModelled(_)) => Ok(url.to_string()),
        Err(FormatError::KeyError(key)) => Err(SphinxCrash::python(
            "KeyError",
            crate::utils::py_repr_str(&key),
            location,
        )),
        Err(FormatError::TypeError(crash) | FormatError::Crash(crash)) => {
            Err(SphinxCrash { location, ..crash })
        }
    }
}

/// Sphinx's `Manpage` (`SP/sphinx/roles.py:531-550`), a `ReferenceRole`:
/// a `manpage` node classed with the role's name, whose `path`, `page`
/// and `section` come from the target, whitespace collapsed; its text the
/// title, less the `!` of a disabled reference; with `manpages_url` set
/// and the reference not disabled, the text in a `reference` to the URL
/// the page formats ([RC §3.7]).
pub(crate) fn manpage(
    call: &RoleCall<'_>,
    manpages_url: Option<&str>,
    location: LogLocation,
) -> Result<Node, SphinxCrash> {
    let parts = ReferenceParts::parse(call.escaped);
    let info = manpage_info(&collapse_whitespace(&parts.target));
    let text = if parts.disabled {
        parts.title.chars().skip(1).collect()
    } else {
        parts.title.clone()
    };
    let mut node = element("manpage", call, vec![call.name.to_string()]);
    node.set("path", AttrValue::Str(info.0.clone()));
    node.set("page", AttrValue::Str(info.1.clone()));
    // A section of `None` prints as pformat prints `None`.
    node.set(
        "section",
        AttrValue::Str(info.2.clone().unwrap_or_else(|| "True".to_string())),
    );
    node.set("xml:space", AttrValue::Str("preserve".to_string()));
    match manpages_url.filter(|url| !url.is_empty() && !parts.disabled) {
        Some(url) => {
            let mut reference = Node::elem(kinds::REFERENCE, call.span);
            reference.attrs.classes = vec![call.name.to_string()];
            reference.set(
                "refuri",
                AttrValue::Str(format_manpages_url(url, &info, location)?),
            );
            push_text(&mut reference, &text, call);
            node.children.push(reference);
        }
        None => node.children.push(Node::text_node(text, call.span)),
    }
    Ok(node)
}

/// Sphinx's `code_role` (`SP/sphinx/roles.py:577-605`), the canonical
/// `code` of a Sphinx build: a `literal` classed `code`, `highlight` with
/// a language, the call's classes, then the language unless a class names
/// it, with its `language` attribute, empty with none, over the
/// null-escaped text — the escapes are gone, where docutils' `code` puts
/// the backslashes back ([RC §3.11]).
pub(crate) fn sphinx_code(call: &RoleCall<'_>) -> Node {
    let language = call.options.language.clone().unwrap_or_default();
    let mut classes = vec!["code".to_string()];
    if !language.is_empty() {
        classes.push("highlight".to_string());
    }
    classes.extend(call.options.classes.iter().flatten().cloned());
    if !language.is_empty() && !classes.contains(&language) {
        classes.push(language.clone());
    }
    let mut node = element(kinds::LITERAL, call, classes);
    node.set("language", AttrValue::Str(language));
    node.children
        .push(Node::text_node_escaped(call.escaped, call.span));
    node
}

/// The index entry types `process_index_entry` no longer accepts
/// (`SP/sphinx/util/nodes.py:441-460`): the `ValueError` it raises for
/// `entry`, `None` for any other.
pub(crate) fn removed_index_entry(entry: &str) -> Option<String> {
    let entry = entry.trim_matches(crate::utils::py_isspace);
    let entry = match entry.strip_prefix('!') {
        Some(rest) => rest.trim_start_matches(crate::utils::py_isspace),
        None => entry,
    };
    let removed = [
        "module",
        "keyword",
        "operator",
        "object",
        "exception",
        "statement",
        "builtin",
    ];
    let index_type = removed
        .into_iter()
        .find(|index_type| entry.starts_with(&format!("{index_type}:")))?;
    let value = entry[index_type.len() + 1..].trim_matches(crate::utils::py_isspace);
    let value = if index_type == "builtin" {
        format!("built-in function; {value}")
    } else {
        format!("{index_type}; {value}")
    };
    Some(format!(
        "{} is no longer supported for index entries (from entry {}). Use 'pair: {value}' \
         instead.",
        crate::utils::py_repr_str(index_type),
        crate::utils::py_repr_str(entry)
    ))
}

/// Sphinx's `IndexRole` (`SP/sphinx/domains/index.py:97-117`), a
/// `ReferenceRole`: a fresh `index-N` serial, then an `index` node
/// holding the entries — an explicit target is a full entry
/// (`process_index_entry`), an implicit one a single entry, main when it
/// starts with `!` — a `target` holding the id, which the role does not
/// register (`note_explicit_target` is the directive's), and the title as
/// text ([RC §3.9]). A removed entry type in an explicit target raises
/// `ValueError` out of the role: a crash (decision 13).
pub(crate) fn index_role(
    call: &RoleCall<'_>,
    state: &mut DocumentState,
    location: LogLocation,
) -> Result<Vec<Node>, SphinxCrash> {
    let target_id = format!("index-{}", state.new_index_serialno());
    let (title, entries) = match explicit_title(call.escaped) {
        Some((title, target)) => {
            if let Some(message) = removed_index_entry(&target) {
                return Err(SphinxCrash::python("ValueError", message, location));
            }
            (
                title,
                crate::rst::block::process_index_entry(&target, &target_id),
            )
        }
        None => {
            let text = unescape(call.escaped, false);
            match text.strip_prefix('!') {
                Some(rest) => (
                    rest.to_string(),
                    vec![crate::rst::block::index_entry_tuple(
                        "single", rest, &target_id, "main", None,
                    )],
                ),
                None => (
                    text.clone(),
                    vec![crate::rst::block::index_entry_tuple(
                        "single", &text, &target_id, "", None,
                    )],
                ),
            }
        }
    };
    let mut index = Node::elem("index", call.span);
    index.set("entries", AttrValue::List(entries));
    let mut target = Node::elem(kinds::TARGET, call.span);
    target.attrs.ids.push(target_id);
    Ok(vec![index, target, Node::text_node(title, call.span)])
}

/// One of the roles `PEP`, `RFC`, `CVE` and `CWE` (`SP/sphinx/roles.py:200-384`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExternalLink {
    Pep,
    Rfc,
    Cve,
    Cwe,
}

/// `_format_rfc_target` (`SP/sphinx/roles.py:370-384`): `RFC <number>
/// <Anchor> <rest>` for an `appendix`, `page` or `section` anchor, `RFC
/// <target>` for any other.
fn format_rfc_target(target: &str) -> String {
    let (number, anchor) = target.split_once('#').unwrap_or((target, ""));
    if !anchor.is_empty() {
        let (first, remaining) = anchor.split_once('-').unwrap_or((anchor, ""));
        if let Some(title) = match first {
            "appendix" => Some("Appendix"),
            "page" => Some("Page"),
            "section" => Some("Section"),
            _ => None,
        } {
            return if remaining.is_empty() {
                format!("RFC {number} {title}")
            } else {
                format!("RFC {number} {title} {remaining}")
            };
        }
    }
    format!("RFC {target}")
}

/// The `PEP`, `RFC`, `CVE` and `CWE` roles (`SP/sphinx/roles.py:200-384`),
/// `ReferenceRole`s: a fresh `index-N` serial and the `target` holding it,
/// which the role registers (`note_explicit_target`), both before the
/// number is checked; then the `index` entry, the target and a `reference`
/// wrapping a `strong` title, or — when `int()` rejects the number — the
/// role's error, which the caller reports over a `problematic` of the raw
/// text ([RC §3.10]).
fn external_link(
    kind: ExternalLink,
    call: &RoleCall<'_>,
    state: &mut DocumentState,
) -> Result<Vec<Node>, String> {
    use super::docutils::PyInt;
    let target_id = format!("index-{}", state.new_index_serialno());
    state.ids.register(&target_id, None);
    let parts = ReferenceParts::parse(call.escaped);
    let target = parts.target.as_str();
    let (number, anchor) = match target.split_once('#') {
        Some((number, anchor)) => (number, Some(anchor)),
        None => (target, None),
    };
    let with_anchor = |uri: String| match anchor {
        Some(anchor) => format!("{uri}#{anchor}"),
        None => uri,
    };
    let int = || PyInt::parse(number);
    let (class, index_text, refuri, title, error) = match kind {
        ExternalLink::Pep => (
            "pep",
            format!("Python Enhancement Proposals; PEP {target}"),
            int()
                .map(|n| with_anchor(format!("https://peps.python.org/pep-{}/", n.zero_padded(4)))),
            format!("PEP {}", parts.title),
            format!("invalid PEP number {target}"),
        ),
        ExternalLink::Rfc => {
            let formatted = format_rfc_target(target);
            (
                "rfc",
                format!("RFC; {formatted}"),
                int().map(|n| {
                    with_anchor(format!(
                        "https://datatracker.ietf.org/doc/html/rfc{}.html",
                        n.display()
                    ))
                }),
                formatted,
                format!("invalid RFC number {target}"),
            )
        }
        ExternalLink::Cve => (
            "cve",
            format!("Common Vulnerabilities and Exposures; CVE {target}"),
            Some(with_anchor(format!(
                "https://www.cve.org/CVERecord?id=CVE-{number}"
            ))),
            format!("CVE {}", parts.title),
            format!("invalid CVE number {target}"),
        ),
        ExternalLink::Cwe => (
            "cwe",
            format!("Common Weakness Enumeration; CWE {target}"),
            int().map(|n| {
                with_anchor(format!(
                    "https://cwe.mitre.org/data/definitions/{}.html",
                    n.display()
                ))
            }),
            format!("CWE {}", parts.title),
            format!("invalid CWE number {target}"),
        ),
    };
    let Some(refuri) = refuri else {
        return Err(error);
    };
    let mut index = Node::elem("index", call.span);
    index.set(
        "entries",
        AttrValue::List(vec![crate::rst::block::index_entry_tuple(
            "single",
            &index_text,
            &target_id,
            "",
            None,
        )]),
    );
    let mut target_node = Node::elem(kinds::TARGET, call.span);
    target_node.attrs.ids.push(target_id);
    let mut reference = Node::elem(kinds::REFERENCE, call.span);
    reference.attrs.classes.push(class.to_string());
    reference.set("internal", AttrValue::Int(0));
    reference.set("refuri", AttrValue::Str(refuri));
    let title = if parts.explicit { parts.title } else { title };
    let mut strong = Node::elem(kinds::STRONG, call.span);
    strong.set_rawsource(&title);
    push_text(&mut strong, &title, call);
    reference.children.push(strong);
    Ok(vec![index, target_node, reference])
}

/// Sphinx's `PEP` role ([`external_link`]): a reference to the PEP the
/// target numbers, `pep_base_url` + `pep-%04d/`.
pub(crate) fn pep(call: &RoleCall<'_>, state: &mut DocumentState) -> Result<Vec<Node>, String> {
    external_link(ExternalLink::Pep, call, state)
}

/// Sphinx's `RFC` role ([`external_link`]): a reference to the RFC the
/// target numbers, `rfc_base_url` + `rfc%d.html`, its anchor formatted
/// into the title.
pub(crate) fn rfc(call: &RoleCall<'_>, state: &mut DocumentState) -> Result<Vec<Node>, String> {
    external_link(ExternalLink::Rfc, call, state)
}

/// Sphinx's `CVE` role ([`external_link`]).
pub(crate) fn cve(call: &RoleCall<'_>, state: &mut DocumentState) -> Result<Vec<Node>, String> {
    external_link(ExternalLink::Cve, call, state)
}

/// Sphinx's `CWE` role ([`external_link`]).
pub(crate) fn cwe(call: &RoleCall<'_>, state: &mut DocumentState) -> Result<Vec<Node>, String> {
    external_link(ExternalLink::Cwe, call, state)
}

/// The `download` role's half of the read ([RC §3.8]): `XRefRole(nodeclass=
/// addnodes.download_reference)`, registered without a domain
/// ([`XrefRole::DOWNLOAD`]); the `DownloadFileCollector` stamps the node
/// in stage M.
pub(crate) fn download(
    call: &RoleCall<'_>,
    env: &XrefEnv<'_>,
    state: &mut DocumentState,
) -> Vec<Node> {
    xref(call, &XrefRole::DOWNLOAD, env, state, None)
}

/// `ws_re.sub(' ', text)` — the base `XRefRole.process_link` munging
/// (`sphinx/roles.py`): every whitespace run becomes one space. `ws_re`
/// is `re.compile(r'\s+')` (`sphinx/util/__init__.py:17`), and Python's
/// `\s` is `str.isspace` — which admits `\x1c`-`\x1f` where Rust's
/// `is_whitespace` does not ([`crate::utils::py_isspace`]; probed:
/// `:doc:`a\x1fb`` reaches sphinx's resolver as `'a b'`).
pub(crate) fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_ws = false;
    for c in text.chars() {
        if crate::utils::py_isspace(c) {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
}

/// Which `process_link` an `XRefRole` runs ([`xref`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkProcessing {
    /// `XRefRole.process_link`: the target's whitespace runs collapsed.
    Base,
    /// `AnyXRefRole`: the base's, then every `ref_context` key copied
    /// onto the node.
    Any,
    /// `OptionXRefRole` (`SP/sphinx/domains/std/__init__.py:351-361`):
    /// `std:program` from the `ref_context`; the target untouched.
    Option,
    /// `TokenXRefRole` (`:703-718`): the target untouched. Its `~`
    /// handling is not modelled.
    Token,
    /// `PyXRefRole` (`SP/sphinx/domains/python/__init__.py:559-585`), and
    /// `_PyDecoXRefRole`, which prefixes `@` to the title (`:588-600`).
    Py { deco: bool },
    /// `JSXRefRole` (`SP/sphinx/domains/javascript.py:386-409`).
    Js,
    /// `CXRefRole` and `CPPXRefRole`, and the c/cpp expression roles,
    /// which are not modelled (M5): the target untouched.
    CFamily,
}

/// What an `XRefRole`'s `result_nodes` does ([`xref`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResultNodes {
    /// `XRefRole.result_nodes`: the node alone.
    Plain,
    /// `EnvVarXRefRole` (`SP/sphinx/domains/std/__init__.py:91-112`): a
    /// reference also indexes the variable.
    EnvVar,
    /// `MathReferenceRole` (`SP/sphinx/domains/math.py:32-41`): `refdomain`
    /// is `math`, on whichever node the role returns.
    Math,
}

/// One `XRefRole` instance: what its constructor and its class give it
/// (`SP/sphinx/roles.py:46-197`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct XrefRole {
    /// `nodeclass`: `pending_xref`, or `download_reference`.
    pub(crate) nodeclass: &'static str,
    /// `innernodeclass`: `literal`, or `inline`.
    pub(crate) innernodeclass: &'static str,
    pub(crate) fix_parens: bool,
    pub(crate) lowercase: bool,
    /// `warn_dangling`, the node's `refwarn`.
    pub(crate) warn_dangling: bool,
    pub(crate) process_link: LinkProcessing,
    pub(crate) result_nodes: ResultNodes,
}

impl XrefRole {
    /// `XRefRole()`: a `pending_xref` over a `literal`, the base class's
    /// `process_link` and `result_nodes`, no option set.
    pub(crate) const fn base() -> XrefRole {
        XrefRole {
            nodeclass: "pending_xref",
            innernodeclass: kinds::LITERAL,
            fix_parens: false,
            lowercase: false,
            warn_dangling: false,
            process_link: LinkProcessing::Base,
            result_nodes: ResultNodes::Plain,
        }
    }

    /// `AnyXRefRole(warn_dangling=True)`, registered as `any`.
    pub(crate) const ANY: XrefRole = XrefRole {
        warn_dangling: true,
        process_link: LinkProcessing::Any,
        ..XrefRole::base()
    };

    /// `XRefRole(nodeclass=addnodes.download_reference)`, registered as
    /// `download`.
    pub(crate) const DOWNLOAD: XrefRole = XrefRole {
        nodeclass: "download_reference",
        ..XrefRole::base()
    };

    /// `MathReferenceRole(warn_dangling=True)`, registered as `eq`.
    pub(crate) const EQ: XrefRole = XrefRole {
        warn_dangling: true,
        result_nodes: ResultNodes::Math,
        ..XrefRole::base()
    };

    /// The role `domain` registers as `name` (one `DomainId::role`
    /// returns): the `roles` table of each domain
    /// (`SP/sphinx/domains/std/__init__.py:748-766`,
    /// `python/__init__.py:755-767`, `javascript.py:434-441`,
    /// `c/__init__.py:809-822`, `cpp/__init__.py:986-1000`, `rst.py:243-246`,
    /// `math.py:61-63`).
    pub(crate) fn of_domain(domain: DomainId, name: &str) -> XrefRole {
        let base = XrefRole::base();
        match domain {
            DomainId::Std => match name {
                "option" => XrefRole {
                    warn_dangling: true,
                    process_link: LinkProcessing::Option,
                    ..base
                },
                "confval" | "keyword" => XrefRole {
                    warn_dangling: true,
                    ..base
                },
                "envvar" => XrefRole {
                    result_nodes: ResultNodes::EnvVar,
                    ..base
                },
                "token" => XrefRole {
                    process_link: LinkProcessing::Token,
                    ..base
                },
                "term" | "doc" => XrefRole {
                    innernodeclass: "inline",
                    warn_dangling: true,
                    ..base
                },
                "ref" => XrefRole {
                    innernodeclass: "inline",
                    lowercase: true,
                    warn_dangling: true,
                    ..base
                },
                "numref" => XrefRole {
                    lowercase: true,
                    warn_dangling: true,
                    ..base
                },
                _ => base,
            },
            DomainId::Py => XrefRole {
                fix_parens: matches!(name, "func" | "meth"),
                process_link: LinkProcessing::Py {
                    deco: name == "deco",
                },
                ..base
            },
            DomainId::Js => XrefRole {
                fix_parens: matches!(name, "func" | "meth"),
                process_link: LinkProcessing::Js,
                ..base
            },
            DomainId::C | DomainId::Cpp => XrefRole {
                fix_parens: name == "func",
                process_link: LinkProcessing::CFamily,
                ..base
            },
            DomainId::Math => XrefRole {
                result_nodes: ResultNodes::Math,
                ..base
            },
            _ => base,
        }
    }
}

/// What an `XRefRole` reads of the document it is in.
#[derive(Clone, Copy)]
pub(crate) struct XrefEnv<'a> {
    /// `env.current_document.docname`, the node's `refdoc`.
    pub(crate) docname: &'a str,
    /// The `env.ref_context` the role reads ([`RefContext`]).
    pub(crate) ref_context: RefContext<'a>,
    /// `config.add_function_parentheses`.
    pub(crate) add_function_parentheses: bool,
}

/// `XRefRole.update_title_and_target` (`roles.py:87-98`), run for the
/// roles constructed with `fix_parens=True`:
///
/// ```python
/// if not self.has_explicit_title:
///     if self.config.add_function_parentheses:
///         if not title.endswith('()'):
///             title += '()'
///     else:
///         title = title.removesuffix('()')
/// target = target.removesuffix('()')
/// ```
///
/// So: an IMPLICIT title ends with exactly one `()` when the config is
/// on and with none when it is off — either way the parens the author
/// wrote are normalised away first, which is why an already
/// parenthesized title is not doubled (probe P3). An EXPLICIT title is
/// untouched (P5). The TARGET loses one trailing pair unconditionally,
/// explicit titles included and under both settings (P3/P4, Q4/Q5) —
/// one pair only, never more (Q10/Q11).
pub(crate) fn update_title_and_target(
    title: String,
    target: String,
    explicit: bool,
    add_function_parentheses: bool,
) -> (String, String) {
    let mut title = title;
    if !explicit {
        if add_function_parentheses {
            if !title.ends_with("()") {
                title.push_str("()");
            }
        } else if let Some(stripped) = title.strip_suffix("()") {
            title = stripped.to_string();
        }
    }
    let target = match target.strip_suffix("()") {
        Some(stripped) => stripped.to_string(),
        None => target,
    };
    (title, target)
}

/// `title.lstrip('.')`, `target.lstrip('~')` and the `~` shortening of
/// `PyXRefRole` and `JSXRefRole`'s `process_link` for an implicit title:
/// one leading `~` on the title reduces it to its last dotted component.
fn shorten_tilde(title: &str, target: &str) -> (String, String) {
    let mut title = title.trim_start_matches('.').to_string();
    let target = target.trim_start_matches('~').to_string();
    if let Some(rest) = title.strip_prefix('~') {
        title = match rest.rfind('.') {
            Some(dot) => rest[dot + 1..].to_string(),
            None => rest.to_string(),
        };
    }
    (title, target)
}

/// `XRefRole.run` (`SP/sphinx/roles.py:100-160`) for `role`, called as
/// `call.name` — `domain:name` for a domain's role, the name written or
/// the default role's for a role registered without a domain — over the
/// interpreted text: a `nodeclass` node with `refdoc`, `refdomain`,
/// `refexplicit`, `reftarget`, `reftype` and `refwarn` wrapping an
/// `innernodeclass` node classed `xref` and the role's domain and type
/// (`xref <type>` for a role registered without a domain); for a
/// disabled reference (`!`), that inner node alone over the text after
/// the `!` (`create_non_xref_node`). `external` is `Some(inventory)` for
/// a node the `:external:` role produced — `Some(None)` when that role
/// named no inventory — which the node carries as `intersphinx` and
/// `inventory` (`_resolve.py:474-483`).
pub(crate) fn xref(
    call: &RoleCall<'_>,
    role: &XrefRole,
    env: &XrefEnv<'_>,
    state: &mut DocumentState,
    external: Option<Option<String>>,
) -> Vec<Node> {
    let name = call.name;
    let (refdomain, reftype, classes) = match name.split_once(':') {
        None => (
            String::new(),
            name.to_string(),
            vec!["xref".to_string(), name.to_string()],
        ),
        Some((domain, reftype)) => (
            domain.to_string(),
            reftype.to_string(),
            vec![
                "xref".to_string(),
                domain.to_string(),
                format!("{domain}-{reftype}"),
            ],
        ),
    };
    let parts = ReferenceParts::parse(call.escaped);
    let inner = |text: String| {
        let mut inner = Node::elem(role.innernodeclass, call.span);
        inner.set_rawsource(call.rawtext);
        inner.attrs.classes = classes.clone();
        push_text(&mut inner, &text, call);
        inner
    };

    if parts.disabled {
        // `create_non_xref_node`: `unescape(self.text[1:])`, and
        // `fix_parens` as if the title were implicit; no `process_link`.
        let text: String = unescape(call.escaped, false).chars().skip(1).collect();
        let text = if role.fix_parens {
            update_title_and_target(text, String::new(), false, env.add_function_parentheses).0
        } else {
            text
        };
        let mut node = inner(text);
        if role.result_nodes == ResultNodes::Math {
            node.set("refdomain", AttrValue::Str("math".to_string()));
        }
        return vec![node];
    }

    // `create_xref_node`.
    let mut target = parts.target;
    let mut title = parts.title;
    if role.lowercase {
        target = target.to_lowercase();
    }
    if role.fix_parens {
        (title, target) =
            update_title_and_target(title, target, parts.explicit, env.add_function_parentheses);
    }
    let mut node = Node::elem(role.nodeclass, call.span);
    node.set_rawsource(call.rawtext);
    node.set("refdoc", AttrValue::Str(env.docname.to_string()));
    node.set("refdomain", AttrValue::Str(refdomain));
    node.set("reftype", AttrValue::Str(reftype));
    node.set("refexplicit", AttrValue::Int(i64::from(parts.explicit)));
    node.set("refwarn", AttrValue::Int(i64::from(role.warn_dangling)));
    // `node['intersphinx'] = True; node['inventory'] = inv_name`
    // (`_resolve.py:474-483`) — the stamp that sends this node through
    // `IntersphinxRoleResolver` instead of ordinary domain resolution.
    if let Some(inventory) = external {
        node.set("intersphinx", AttrValue::Int(1));
        if let Some(inventory) = inventory {
            node.set("inventory", AttrValue::Str(inventory));
        }
    }

    // `process_link`. A `None` from the `ref_context` prints as pformat
    // prints `None`, `"True"`.
    let none = |value: Option<&str>| AttrValue::Str(value.unwrap_or("True").to_string());
    let ctx = env.ref_context;
    match role.process_link {
        LinkProcessing::Base => target = collapse_whitespace(&target),
        LinkProcessing::Any => {
            target = collapse_whitespace(&target);
            // `refnode.attributes.update(env.ref_context)`: only the keys
            // that EXIST, and the two list values (`py:classes`,
            // `py:modules`) by reference, which the balanced directive
            // nesting has drained back to `[]` — `""` — by the time the
            // doctree is serialized ([`RefContext`]).
            if ctx.py_class_key {
                node.set("py:class", none(ctx.py_class));
            }
            if ctx.py_classes_key {
                node.set("py:classes", AttrValue::Str(String::new()));
            }
            if ctx.py_module_key {
                node.set("py:module", none(ctx.py_module));
            }
            if ctx.py_modules_key {
                node.set("py:modules", AttrValue::Str(String::new()));
            }
            if let Some(program) = ctx.program {
                node.set("std:program", AttrValue::Str(program.to_string()));
            }
        }
        LinkProcessing::Option => node.set("std:program", none(ctx.program)),
        LinkProcessing::Token | LinkProcessing::CFamily => {}
        LinkProcessing::Py { deco } => {
            node.set("py:module", none(ctx.py_module));
            node.set("py:class", none(ctx.py_class));
            if !parts.explicit {
                (title, target) = shorten_tilde(&title, &target);
            }
            // A `.`-prefixed target — explicit titles included — loses
            // the dot and searches more specific namespaces first.
            if let Some(rest) = target.strip_prefix('.') {
                target = rest.to_string();
                node.set("refspecific", AttrValue::Int(1));
            }
            // `_PyDecoXRefRole` prefixes `@` unconditionally.
            if deco {
                title = format!("@{title}");
            }
        }
        LinkProcessing::Js => {
            // The js directives that would set them are not modelled: the
            // `ref_context` never holds `js:object` or `js:module`.
            node.set("js:object", none(None));
            node.set("js:module", none(None));
            if !parts.explicit {
                (title, target) = shorten_tilde(&title, &target);
            }
            if let Some(rest) = target.strip_prefix('.') {
                target = rest.to_string();
                node.set("refspecific", AttrValue::Int(1));
            }
        }
    }
    node.set("reftarget", AttrValue::Str(target.clone()));
    node.children.push(inner(title));

    match role.result_nodes {
        ResultNodes::Plain => vec![node],
        ResultNodes::Math => {
            node.set("refdomain", AttrValue::Str("math".to_string()));
            vec![node]
        }
        // An `:envvar:` reference also indexes the variable, under its
        // bare name and under the `environment variable; %s` heading the
        // `.. envvar::` directive uses, both on a fresh `index-N` target
        // placed just before the reference.
        ResultNodes::EnvVar => {
            let target_id = format!("index-{}", state.new_index_serialno());
            let mut index = Node::elem("index", call.span);
            index.set(
                "entries",
                AttrValue::List(vec![
                    crate::rst::block::index_entry_tuple("single", &target, &target_id, "", None),
                    crate::rst::block::index_entry_tuple(
                        "single",
                        &format!("environment variable; {target}"),
                        &target_id,
                        "",
                        None,
                    ),
                ]),
            );
            let mut target_node = Node::elem(kinds::TARGET, call.span);
            target_node.attrs.ids.push(target_id.clone());
            state.ids.register(&target_id, None);
            vec![index, target_node, node]
        }
    }
}

#[cfg(test)]
mod tests;
