//! A port of docutils' SmartyPants functions, `docutils.utils.smartquotes`
//! (`SP/docutils/utils/smartquotes.py`), which Sphinx's smart-quotes
//! transform hands the text of each block to, and of the one line of
//! `SphinxSmartQuotes.get_tokens` (`SP/sphinx/transforms/__init__.py:411`)
//! that marks escaped characters for them.
//!
//! Every function is pure, text in and text out, and does what its Python
//! original does, Python's character classes included: `\s` is
//! `str.isspace()`, `\w` is `str.isalnum()` or `_` ([`tables::PY_WORD_RUNS`])
//! and `\d` is `str.isdecimal()`, none of which is Rust's predicate of the
//! same name. `tests/fixtures/smartquotes_units.json`, which
//! `tools/gen_smartquotes_fixture.py --units` records from the originals,
//! is what the tests below compare the port with.
//!
//! [`tables::PY_WORD_RUNS`]: super::tables::PY_WORD_RUNS

use std::cmp::Ordering;

use super::tables::{QuoteSet, PY_WORD_RUNS, QUOTES};
use crate::rst::digits::decimal_digit_value;
use crate::utils::py_isspace;

/// A token's kind, `educate_tokens`' `ttype`: text to educate, or text
/// that is only context, which Sphinx's `get_tokens` hands over for a Text
/// node inside a literal, a `math`, a signature and the like
/// (`is_smartquotable`, `SP/sphinx/util/nodes.py:697-716`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// `'plain'`: educated, the backslash escapes [`process_escapes`]
    /// reads included.
    Plain,
    /// `'literal'`: yielded as it is; its last character is the context of
    /// the next token.
    Literal,
}

/// What `educate_tokens` does, as its `attr` string asks
/// (`smartquotes.py:589-617`). Sphinx's default, `smartquotes_action`, is
/// `qDe`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Actions {
    /// `q`: `"` and `'` ([`educate_quotes`]).
    pub quotes: bool,
    /// `b` (1): ``` `` ``` and `''` ([`educate_backticks`]); `B` (2): those,
    /// then `` ` `` and `'` ([`educate_single_backticks`]).
    pub backticks: u8,
    /// `d` (1) and `i` (3): `---` to an en dash and `--` to an em dash
    /// ([`educate_dashes`]); `D` (2): the other way round
    /// ([`educate_dashes_oldschool`]).
    pub dashes: u8,
    /// `e`: `...` and `. . .` ([`educate_ellipses`]).
    pub ellipses: bool,
    /// `w`: `&quot;` to `"` before anything is educated.
    pub convert_quot: bool,
    /// The whole string `-1`: curly quotes, dashes and ellipses to ASCII
    /// ([`stupefy_entities`]).
    pub stupefy: bool,
}

impl Actions {
    /// `educate_tokens`' reading of `attr`: the whole strings `1`, `2` and
    /// `3` turn on quotes, double backticks, ellipses and that dash mode,
    /// and `-1` stupefies; any other string is a set of letters, read in
    /// the order of docutils' `if` chain, so that `B` beats `b` and `i`
    /// beats `D`, which beats `d`, wherever they stand. Other letters, `0`
    /// and `''` turn on nothing, but [`educate_tokens`] still runs the
    /// escapes then.
    pub fn parse(attr: &str) -> Actions {
        let all = |dashes| Actions {
            quotes: true,
            backticks: 1,
            dashes,
            ellipses: true,
            ..Actions::default()
        };
        match attr {
            "1" => all(1),
            "2" => all(2),
            "3" => all(3),
            "-1" => Actions {
                stupefy: true,
                ..Actions::default()
            },
            _ => {
                let has = |letter| attr.contains(letter);
                let backticks = if has('B') { 2 } else { u8::from(has('b')) };
                let dashes = if has('i') {
                    3
                } else if has('D') {
                    2
                } else {
                    u8::from(has('d'))
                };
                Actions {
                    quotes: has('q'),
                    backticks,
                    dashes,
                    ellipses: has('e'),
                    convert_quot: has('w'),
                    stupefy: false,
                }
            }
        }
    }
}

/// `smartchars('')`'s quotes, for a language docutils has none for: the
/// ASCII ones (`smartquotes.py:499-505`).
pub const ASCII_QUOTES: QuoteSet = QuoteSet {
    op: "\"",
    cp: "\"",
    os: "'",
    cs: "'",
};

/// `smartchars.apostrophe`, RIGHT SINGLE QUOTATION MARK.
pub const APOSTROPHE: &str = "\u{2019}";

const EN_DASH: &str = "\u{2013}";
const EM_DASH: &str = "\u{2014}";
const ELLIPSIS: &str = "\u{2026}";

/// `smartquotes.educate_tokens(text_tokens, attr, language)`
/// (`smartquotes.py:565-675`): one output per token, in order. An empty
/// token comes back as it is and leaves the context alone; a literal one
/// comes back as it is and makes its last character the context; a plain
/// one goes through the escapes, `&quot;`, the dashes, the ellipses, the
/// backticks and the quotes, in that order, its quotes decided with the
/// previous token's last character in front of it (a quote there counts
/// as `;`), and then its own last character as it was before any of that
/// is the next token's context.
pub fn educate_tokens(
    tokens: &[(TokenKind, String)],
    actions: &Actions,
    language: &str,
) -> Vec<String> {
    let quotes = quote_set(language);
    let english = language.starts_with("en");
    let mut prev_token_last_char = ' ';
    let mut out = Vec::with_capacity(tokens.len());
    for (kind, text) in tokens {
        let Some(last_char) = text.chars().next_back() else {
            out.push(String::new());
            continue;
        };
        if *kind == TokenKind::Literal {
            prev_token_last_char = last_char;
            out.push(text.clone());
            continue;
        }
        let mut text = process_escapes(text);
        if actions.convert_quot {
            text = text.replace("&quot;", "\"");
        }
        match actions.dashes {
            1 | 3 => text = educate_dashes(&text),
            2 => text = educate_dashes_oldschool(&text),
            _ => {}
        }
        if actions.ellipses {
            text = educate_ellipses(&text);
        }
        if actions.backticks != 0 {
            text = educate_backticks(&text, &quotes);
        }
        if actions.backticks == 2 {
            text = educate_single_backticks(&text, &quotes);
        }
        if actions.quotes {
            // A quote as context would come back as a two-character French
            // quote; `;` stands in for it.
            let context = match prev_token_last_char {
                '"' | '\'' => ';',
                other => other,
            };
            let mut with_context = String::with_capacity(text.len() + context.len_utf8());
            with_context.push(context);
            with_context.push_str(&text);
            let educated = quotes_with(&with_context, &quotes, english);
            // No pass changes a character that is not a quote, so the
            // context comes back first, as it went in.
            debug_assert!(educated.starts_with(context));
            text = educated[context.len_utf8()..].to_string();
        }
        if actions.stupefy {
            text = stupefy_entities(&text, &quotes);
        }
        prev_token_last_char = last_char;
        out.push(restore_escapes(&text));
    }
    out
}

/// `smartquotes.educateQuotes(text, language)` (`smartquotes.py:678-735`):
/// the straight quotes of `text` curled with `language`'s quotes
/// ([`quote_set`]), by the thirteen passes of the original, each over the
/// one before's output. [`educate_tokens`] puts the context character in
/// front of the text it hands over and drops it again.
pub fn educate_quotes(text: &str, language: &str) -> String {
    quotes_with(text, &quote_set(language), language.starts_with("en"))
}

/// [`educate_quotes`] with `language`'s quotes found and its `en` test
/// made. Each pass replaces quotes only, so a pass is a decision about
/// each quote on the pass's input ([`sub_quote`], [`sub_pair`]), which is
/// what Python's leftmost, non-overlapping `re.sub` makes of these
/// patterns.
fn quotes_with(text: &str, q: &QuoteSet, english: bool) -> String {
    if !text.contains(['-', '"', '\'']) {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    // START_SINGLE / START_DOUBLE: a quote first, then punctuation, then a
    // literal backslash and `B` (the raw pattern text `\\B`). The first
    // character `educate_tokens` hands over is its context, never a quote.
    chars = sub_quote(chars, '\'', q.cs, |c, i| i == 0 && starts_closing(c));
    chars = sub_quote(chars, '"', q.cp, |c, i| i == 0 && starts_closing(c));
    // ADJACENT_1 / ADJACENT_2: `"'` and `'"` before a word character.
    chars = sub_pair(chars, ('"', '\''), (q.op, q.os));
    chars = sub_pair(chars, ('\'', '"'), (q.os, q.op));
    // OPEN_SINGLE / OPEN_DOUBLE: after an opening bracket or a dash, and
    // before a space, or punctuation and a space: closing.
    chars = sub_quote(chars, '\'', q.cs, |c, i| {
        before(c, i).is_some_and(|b| is_open(b) || is_dash(b)) && spaced_after(c, i)
    });
    chars = sub_quote(chars, '"', q.cp, |c, i| {
        before(c, i).is_some_and(|b| is_open(b) || is_dash(b)) && spaced_after(c, i)
    });
    // DECADE: `'` before two digits and `s`, in English.
    if english {
        chars = sub_quote(chars, '\'', APOSTROPHE, |c, i| {
            after(c, i, 1).is_some_and(py_is_decimal)
                && after(c, i, 2).is_some_and(py_is_decimal)
                && after(c, i, 3) == Some('s')
        });
    }
    // OPENING_SECONDARY: after a separator, an opening bracket or a dash,
    // and before a word character or punctuation.
    chars = sub_quote(chars, '\'', q.os, opens_word);
    // APOSTROPHE: between word characters, unless the closing secondary
    // quote is the apostrophe already.
    if q.cs != APOSTROPHE {
        chars = sub_quote(chars, '\'', APOSTROPHE, |c, i| {
            before(c, i).is_some_and(py_is_word) && after(c, i, 1).is_some_and(py_is_word)
        });
    }
    // CLOSING_SECONDARY: not after whitespace; the rest open.
    chars = sub_quote(chars, '\'', q.cs, |c, i| {
        !before(c, i).is_some_and(py_isspace)
    });
    chars = sub_quote(chars, '\'', q.os, |_, _| true);
    // OPENING_PRIMARY, as OPENING_SECONDARY.
    chars = sub_quote(chars, '"', q.op, opens_word);
    // CLOSING_PRIMARY: not after whitespace, or before whitespace; the
    // rest open.
    chars = sub_quote(chars, '"', q.cp, |c, i| {
        !before(c, i).is_some_and(py_isspace) || after(c, i, 1).is_some_and(py_isspace)
    });
    chars = sub_quote(chars, '"', q.op, |_, _| true);
    chars.into_iter().collect()
}

/// One `re.sub` pass that replaces a lone `quote` by `with` where `hit`
/// holds at its index. The patterns these passes port consume the quote
/// and at most the one character before it, which they put back (`\1`)
/// and which is never a quote, so no two matches overlap and each quote
/// can be decided on the pass's input alone.
fn sub_quote(
    chars: Vec<char>,
    quote: char,
    with: &str,
    hit: impl Fn(&[char], usize) -> bool,
) -> Vec<char> {
    if !chars.contains(&quote) {
        return chars;
    }
    let mut out = Vec::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == quote && hit(&chars, i) {
            out.extend(with.chars());
        } else {
            out.push(c);
        }
    }
    out
}

/// `ADJACENT_1`/`ADJACENT_2` (`'"\'(?=\w)'`, `'\'"(?=\w)'`): the pair
/// `first, second` before a word character becomes `with`. The pair's
/// second character is not its first, so the leftmost match is the first
/// such pair from the left and the scan goes on after it.
fn sub_pair(chars: Vec<char>, pair: (char, char), with: (&str, &str)) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == pair.0
            && after(&chars, i, 1) == Some(pair.1)
            && after(&chars, i, 2).is_some_and(py_is_word)
        {
            out.extend(with.0.chars());
            out.extend(with.1.chars());
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// The character before index `i`, if any.
fn before(chars: &[char], i: usize) -> Option<char> {
    i.checked_sub(1).map(|j| chars[j])
}

/// The character `k` places after index `i`, if any.
fn after(chars: &[char], i: usize, k: usize) -> Option<char> {
    chars.get(i + k).copied()
}

/// `START_*`'s lookahead from index 0: punctuation, a backslash and `B`.
fn starts_closing(chars: &[char]) -> bool {
    after(chars, 0, 1).is_some_and(is_punct)
        && after(chars, 0, 2) == Some('\\')
        && after(chars, 0, 3) == Some('B')
}

/// `OPEN_*`'s lookahead, `(?=punct? )`: a space, or punctuation and then a
/// space (the space U+0020 itself, not `\s`).
fn spaced_after(chars: &[char], i: usize) -> bool {
    after(chars, i, 1) == Some(' ')
        || (after(chars, i, 1).is_some_and(is_punct) && after(chars, i, 2) == Some(' '))
}

/// `OPENING_SECONDARY`/`OPENING_PRIMARY`: after a separator, an opening
/// bracket or a dash, and before a word character or punctuation.
fn opens_word(chars: &[char], i: usize) -> bool {
    before(chars, i).is_some_and(|b| is_sep(b) || is_open(b) || is_dash(b))
        && after(chars, i, 1).is_some_and(|a| py_is_word(a) || is_punct(a))
}

/// `smartquotes.educateDashes` (`:769-778`), which
/// `educateDashesOldSchoolInverted` (`:794-810`) repeats: `---` to an en
/// dash, then `--` to an em dash.
pub fn educate_dashes(text: &str) -> String {
    text.replace("---", EN_DASH).replace("--", EM_DASH)
}

/// `smartquotes.educateDashesOldSchool` (`:781-791`): `---` to an em dash,
/// then `--` to an en dash.
pub fn educate_dashes_oldschool(text: &str) -> String {
    text.replace("---", EM_DASH).replace("--", EN_DASH)
}

/// `smartquotes.educateEllipses` (`:813-825`): `...`, then `. . .`, to an
/// ellipsis.
pub fn educate_ellipses(text: &str) -> String {
    text.replace("...", ELLIPSIS).replace(". . .", ELLIPSIS)
}

/// `smartquotes.educateBackticks` (`:738-750`): ``` `` ``` to the opening
/// primary quote, then `''` to the closing one.
pub fn educate_backticks(text: &str, q: &QuoteSet) -> String {
    text.replace("``", q.op).replace("''", q.cp)
}

/// `smartquotes.educateSingleBackticks` (`:753-766`): `` ` `` to the
/// opening secondary quote, then `'` to the closing one.
pub fn educate_single_backticks(text: &str, q: &QuoteSet) -> String {
    text.replace('`', q.os).replace('\'', q.cs)
}

/// `smartquotes.stupefyEntities` (`:828-847`): the dashes, the language's
/// quotes and the ellipsis back to ASCII, in that order.
pub fn stupefy_entities(text: &str, q: &QuoteSet) -> String {
    text.replace(EN_DASH, "-")
        .replace(EM_DASH, "--")
        .replace(q.os, "'")
        .replace(q.cs, "'")
        .replace(q.op, "\"")
        .replace(q.cp, "\"")
        .replace(ELLIPSIS, "...")
}

/// `processEscapes`' pairs (`:866-871`), in its order.
const ESCAPES: [(&str, &str); 6] = [
    ("\\\\", "&#92;"),
    ("\\\"", "&#34;"),
    ("\\'", "&#39;"),
    ("\\.", "&#46;"),
    ("\\-", "&#45;"),
    ("\\`", "&#96;"),
];

/// `smartquotes.processEscapes(text)` (`:850-879`): each of
/// `` \\ \" \' \. \- \` `` to its numeric entity, pair by pair, so that
/// the passes do not see the character. It takes every such backslash,
/// not only those an escape marker stands before
/// ([`insert_escape_backslashes`]).
pub fn process_escapes(text: &str) -> String {
    ESCAPES
        .iter()
        .fold(text.to_string(), |text, (escape, entity)| {
            text.replace(escape, entity)
        })
}

/// `processEscapes(text, restore=True)`: each entity of [`process_escapes`]
/// back to the bare character, without its backslash, pair by pair. It
/// decodes such entities wherever they come from, the source text
/// included.
pub fn restore_escapes(text: &str) -> String {
    ESCAPES
        .iter()
        .fold(text.to_string(), |text, (escape, entity)| {
            text.replace(entity, &escape[1..])
        })
}

/// What Sphinx's `get_tokens` does to a plain Text node's data before
/// [`educate_tokens`] reads it,
/// ``re.sub(r'(?<=\x00)([-\\\'".`])', r'\\\1', text)``
/// (`SP/sphinx/transforms/__init__.py:411`): a backslash after each
/// `\0` escape marker that stands before `-`, `\`, `'`, `"`, `.` or a
/// backtick, so that [`process_escapes`] keeps the escaped character
/// straight.
pub fn insert_escape_backslashes(data: &str) -> String {
    let mut out = String::with_capacity(data.len());
    let mut prev = None;
    for c in data.chars() {
        if prev == Some('\0') && matches!(c, '-' | '\\' | '\'' | '"' | '.' | '`') {
            out.push('\\');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// `smartchars(language)`'s quotes (`smartquotes.py:499-505`): the table's
/// entry for `language` lowercased, or [`ASCII_QUOTES`].
pub fn quote_set(language: &str) -> QuoteSet {
    table_entry(&language.to_lowercase()).unwrap_or(ASCII_QUOTES)
}

/// `tag in smartchars.quotes`: whether docutils has quotes for `tag` as it
/// is spelled.
pub fn has_quotes(tag: &str) -> bool {
    table_entry(tag).is_some()
}

fn table_entry(tag: &str) -> Option<QuoteSet> {
    QUOTES
        .binary_search_by(|(key, _)| (*key).cmp(tag))
        .ok()
        .map(|index| QUOTES[index].1)
}

/// `docutils.utils.normalize_language_tag(tag)`
/// (`SP/docutils/utils/__init__.py:741-765`): the tag lowercased with `-`
/// as `_`, an ASCII letter or digit between two `_` joined to what follows
/// it (a singleton marks the next subtag as non-standard), and then the
/// base followed by every combination of the other subtags, the longest
/// first and each length in `itertools.combinations` order, joined with
/// `-`, and the base alone last.
pub fn normalize_language_tag(tag: &str) -> Vec<String> {
    let tag = join_singletons(&tag.to_lowercase().replace('-', "_"));
    let mut subtags = tag.split('_');
    let base = subtags.next().unwrap_or_default();
    let subtags: Vec<&str> = subtags.collect();
    let mut taglist = Vec::new();
    for n in (1..=subtags.len()).rev() {
        for combination in combinations(&subtags, n) {
            let mut parts = vec![base];
            parts.extend(combination);
            taglist.push(parts.join("-"));
        }
    }
    taglist.push(base.to_string());
    taglist
}

/// `re.sub(r'_([a-zA-Z0-9])_', r'_\1-', tag)`: left to right, a match's
/// closing `_` not reused.
fn join_singletons(tag: &str) -> String {
    let chars: Vec<char> = tag.chars().collect();
    let mut out = String::with_capacity(tag.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '_'
            && after(&chars, i, 1).is_some_and(|c| c.is_ascii_alphanumeric())
            && after(&chars, i, 2) == Some('_')
        {
            out.push('_');
            out.push(chars[i + 1]);
            out.push('-');
            i += 3;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// `itertools.combinations(items, n)`, in its order: index tuples in
/// lexicographic order.
fn combinations<'a>(items: &[&'a str], n: usize) -> Vec<Vec<&'a str>> {
    let mut out = Vec::new();
    if n > items.len() {
        return out;
    }
    let mut indices: Vec<usize> = (0..n).collect();
    loop {
        out.push(indices.iter().map(|&index| items[index]).collect());
        let Some(i) = (0..n).rev().find(|&i| indices[i] != i + items.len() - n) else {
            return out;
        };
        indices[i] += 1;
        for j in i + 1..n {
            indices[j] = indices[j - 1] + 1;
        }
    }
}

/// `punct` (`smartquotes.py:512`): ASCII punctuation but `&`, and the
/// space, which the class spells twice.
fn is_punct(c: char) -> bool {
    c == ' ' || (c.is_ascii_punctuation() && c != '&')
}

/// `open`: an opening bracket.
fn is_open(c: char) -> bool {
    matches!(c, '(' | '[' | '{')
}

/// `dash`: the hyphen and the en and em dashes.
fn is_dash(c: char) -> bool {
    matches!(c, '-' | '\u{2013}' | '\u{2014}')
}

/// `sep`: Python's `\s`, ZERO WIDTH SPACE and ZERO WIDTH NON-JOINER.
fn is_sep(c: char) -> bool {
    py_isspace(c) || matches!(c, '\u{200B}' | '\u{200C}')
}

/// Python's `\w` on a `str` pattern: `str.isalnum()` or `_`.
pub(crate) fn py_is_word(c: char) -> bool {
    let cp = u32::from(c);
    PY_WORD_RUNS
        .binary_search_by(|&(first, last)| {
            if last < cp {
                Ordering::Less
            } else if first > cp {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

/// Python's `\d` on a `str` pattern: `str.isdecimal()`.
fn py_is_decimal(c: char) -> bool {
    decimal_digit_value(c).is_some()
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    /// `tests/fixtures/smartquotes_units.json`, which
    /// `tools/gen_smartquotes_fixture.py --units` records from docutils
    /// 0.22.4 and Sphinx 9.1.0.
    fn fixture() -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/smartquotes_units.json");
        let text = std::fs::read_to_string(path).expect("fixture readable");
        serde_json::from_str(&text).expect("fixture parses")
    }

    fn str_of(value: &Value) -> &str {
        value
            .as_str()
            .unwrap_or_else(|| panic!("a string: {value}"))
    }

    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .unwrap_or_else(|| panic!("a list: {value}"))
            .iter()
            .map(|item| str_of(item).to_string())
            .collect()
    }

    fn rows(value: &Value) -> &Vec<Value> {
        value
            .as_array()
            .unwrap_or_else(|| panic!("a list of rows: {value}"))
    }

    /// A row's `[[kind, text], ...]`.
    fn tokens(value: &Value) -> Vec<(TokenKind, String)> {
        rows(value)
            .iter()
            .map(|token| {
                let kind = match str_of(&token[0]) {
                    "plain" => TokenKind::Plain,
                    "literal" => TokenKind::Literal,
                    other => panic!("token kind {other:?}"),
                };
                (kind, str_of(&token[1]).to_string())
            })
            .collect()
    }

    /// A random section's settings, `[[attr, language], ...]`.
    fn settings(value: &Value) -> Vec<(String, String)> {
        rows(value)
            .iter()
            .map(|pair| (str_of(&pair[0]).to_string(), str_of(&pair[1]).to_string()))
            .collect()
    }

    /// `educate_tokens` and `educateQuotes` as docutils 0.22.4 runs them,
    /// row by row: the research probe's inputs (one token under each probe
    /// language, the 16 actions, the multi-token context table),
    /// `educateQuotes` called directly, 2,000 random strings under the 16
    /// actions and under `qDe` in the languages whose quotes take another
    /// path, and 500 random sequences of plain and literal tokens. A random
    /// row's `null` output is its input, unchanged.
    #[test]
    fn educate_matches_docutils_units() {
        let fixture = fixture();
        let mut checked = 0usize;
        let mut mismatches = Vec::new();
        let mut check = |tokens: &[(TokenKind, String)],
                         attr: &str,
                         language: &str,
                         expected: Vec<String>| {
            checked += 1;
            let ours = educate_tokens(tokens, &Actions::parse(attr), language);
            if ours != expected {
                mismatches.push(format!(
                    "attr {attr:?}, language {language:?}, tokens {tokens:?}:\n  ours    {ours:?}\n  docutils {expected:?}"
                ));
            }
        };
        for row in rows(&fixture["educate_tokens"]) {
            check(
                &tokens(&row["tokens"]),
                str_of(&row["attr"]),
                str_of(&row["language"]),
                strings(&row["out"]),
            );
        }
        let random_settings = settings(&fixture["random"]["settings"]);
        let actions = strings(&fixture["actions"]);
        assert!(
            actions
                .iter()
                .all(|attr| random_settings.contains(&(attr.clone(), "en".to_string()))),
            "every action under en"
        );
        let random = rows(&fixture["random"]["rows"]);
        assert_eq!(random.len(), 2000, "the random strings");
        for row in random {
            let tokens = tokens(&row["tokens"]);
            let out = rows(&row["out"]);
            assert_eq!(out.len(), random_settings.len());
            for ((attr, language), expected) in random_settings.iter().zip(out) {
                let expected = match expected.as_str() {
                    Some(text) => text.to_string(),
                    None => tokens[0].1.clone(),
                };
                check(&tokens, attr, language, vec![expected]);
            }
        }
        let context_settings = settings(&fixture["random_context"]["settings"]);
        for row in rows(&fixture["random_context"]["rows"]) {
            let tokens = tokens(&row["tokens"]);
            let unchanged: Vec<String> = tokens.iter().map(|(_, text)| text.clone()).collect();
            let out = rows(&row["out"]);
            assert_eq!(out.len(), context_settings.len());
            for ((attr, language), expected) in context_settings.iter().zip(out) {
                let expected = if expected.is_null() {
                    unchanged.clone()
                } else {
                    strings(expected)
                };
                check(&tokens, attr, language, expected);
            }
        }
        for row in rows(&fixture["educate_quotes"]) {
            checked += 1;
            let (text, language) = (str_of(&row["text"]), str_of(&row["language"]));
            let ours = educate_quotes(text, language);
            let expected = str_of(&row["out"]);
            if ours != expected {
                mismatches.push(format!(
                    "educateQuotes({text:?}, {language:?}):\n  ours    {ours:?}\n  docutils {expected:?}"
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "{} of {checked} rows differ from docutils; the first ones:\n{}",
            mismatches.len(),
            mismatches[..mismatches.len().min(12)].join("\n")
        );
        assert!(checked >= 44_000, "only {checked} rows checked");
    }

    /// docutils' `normalize_language_tag` and the tag SmartQuotes picks from
    /// its list, and `smartchars(language)` for every language of the table,
    /// cased variants and unknown tags (ASCII quotes).
    #[test]
    fn language_tags_and_quote_sets_match_docutils() {
        let fixture = fixture();
        for row in rows(&fixture["normalize_language_tag"]) {
            let tag = str_of(&row["tag"]);
            let tags = normalize_language_tag(tag);
            assert_eq!(
                tags,
                strings(&row["tags"]),
                "normalize_language_tag({tag:?})"
            );
            let pick = tags.iter().find(|tag| has_quotes(tag)).map(String::as_str);
            assert_eq!(pick, row["quotes"].as_str(), "the quotes {tag:?} finds");
        }
        let sets = rows(&fixture["quote_sets"]);
        for row in sets {
            let language = str_of(&row["language"]);
            assert_eq!(
                has_quotes(language),
                row["known"].as_bool().expect("known"),
                "{language:?} in smartchars.quotes"
            );
            let set = quote_set(language);
            assert_eq!(
                [set.op, set.cp, set.os, set.cs]
                    .map(str::to_string)
                    .to_vec(),
                strings(&row["quotes"]),
                "smartchars({language:?})"
            );
        }
        assert!(sets.len() > QUOTES.len(), "every language and then some");
        assert_eq!(QUOTES.len(), 75);
        assert!(
            QUOTES.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "sorted, unique"
        );
    }

    /// `combinations` against Python 3.12's `itertools.combinations`:
    /// `list(combinations("abc", 2))`, `list(combinations("ab", 0))` and
    /// `list(combinations("ab", 3))`. `normalize_language_tag` never asks for
    /// more items than it has, so only this test pins the empty result.
    #[test]
    fn combinations_follow_itertools() {
        assert_eq!(
            combinations(&["a", "b", "c"], 2),
            vec![vec!["a", "b"], vec!["a", "c"], vec!["b", "c"]]
        );
        assert_eq!(combinations(&["a", "b"], 0), vec![Vec::<&str>::new()]);
        assert!(combinations(&["a", "b"], 3).is_empty());
    }

    /// `processEscapes` and its restore, and the backslash Sphinx's
    /// `get_tokens` inserts after a `\x00` escape marker (a real
    /// `SphinxSmartQuotes.get_tokens` call made each row).
    #[test]
    fn escape_helpers_match_docutils_and_sphinx() {
        let fixture = fixture();
        for row in rows(&fixture["escapes"]) {
            let text = str_of(&row["text"]);
            assert_eq!(process_escapes(text), str_of(&row["escaped"]), "{text:?}");
            assert_eq!(restore_escapes(text), str_of(&row["restored"]), "{text:?}");
        }
        for row in rows(&fixture["insert_escape_backslashes"]) {
            let data = str_of(&row["data"]);
            assert_eq!(
                insert_escape_backslashes(data),
                str_of(&row["out"]),
                "{data:?}"
            );
        }
    }

    /// A list of `[first, last]` code point runs.
    fn code_point_runs(value: &Value) -> Vec<(u32, u32)> {
        rows(value)
            .iter()
            .map(|run| {
                let at = |i: usize| {
                    u32::try_from(run[i].as_u64().expect("a code point")).expect("in range")
                };
                (at(0), at(1))
            })
            .collect()
    }

    /// The character classes the regular expressions of `educateQuotes`
    /// use, over every code point: `punct` (ASCII punctuation less `&`,
    /// plus the space), `open`, `dash`, `sep` (Python's `\s`, ZWSP and
    /// ZWNJ), `\s`, `\d` and `\w`, each of which the generator read off the
    /// compiled patterns. `\w` is Python's `str.isalnum()`, which a
    /// combining mark Unicode calls alphabetic (U+0345) is not, unlike
    /// Rust's `char::is_alphanumeric`.
    #[test]
    fn character_classes_match_python_re() {
        let fixture = fixture();
        let classes = &fixture["char_classes"];
        let (punct, open, dash) = (
            str_of(&classes["punct"]),
            str_of(&classes["open"]),
            str_of(&classes["dash"]),
        );
        let (sep, space) = (str_of(&classes["sep"]), str_of(&classes["space"]));
        let decimal = code_point_runs(&classes["decimal_runs"]);
        let word = code_point_runs(&classes["word_runs"]);
        assert_eq!(
            PY_WORD_RUNS,
            word.as_slice(),
            "tables.rs is the fixture's \\w"
        );
        // Membership by a sweep over the sorted runs, as the code points
        // ascend: a lookup independent of the binary search under test.
        let mut sweeps = [(decimal.as_slice(), 0usize), (word.as_slice(), 0usize)];
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let cp = u32::from(c);
            let [in_decimal, in_word] = sweeps.each_mut().map(|(runs, next)| {
                while runs.get(*next).is_some_and(|&(_, last)| last < cp) {
                    *next += 1;
                }
                runs.get(*next).is_some_and(|&(first, _)| first <= cp)
            });
            assert_eq!(is_punct(c), punct.contains(c), "punct {c:?}");
            assert_eq!(is_open(c), open.contains(c), "open {c:?}");
            assert_eq!(is_dash(c), dash.contains(c), "dash {c:?}");
            assert_eq!(is_sep(c), sep.contains(c), "sep {c:?}");
            assert_eq!(py_isspace(c), space.contains(c), "\\s {c:?}");
            assert_eq!(py_is_decimal(c), in_decimal, "\\d {c:?}");
            assert_eq!(py_is_word(c), in_word, "\\w {c:?}");
        }
        assert!('\u{345}'.is_alphanumeric() && !py_is_word('\u{345}'));
    }

    /// `educate_tokens`' `attr` chain (`smartquotes.py:589-617`): the
    /// whole strings `1`, `2`, `3` and `-1` are presets; anything else is a
    /// set of letters, where `B` beats `b` and `i` beats `D` beats `d`
    /// whatever their order.
    #[test]
    fn actions_parse_follows_the_attr_chain() {
        let all = |dashes| Actions {
            quotes: true,
            backticks: 1,
            dashes,
            ellipses: true,
            ..Actions::default()
        };
        assert_eq!(Actions::parse("1"), all(1));
        assert_eq!(Actions::parse("2"), all(2));
        assert_eq!(Actions::parse("3"), all(3));
        assert_eq!(
            Actions::parse("-1"),
            Actions {
                stupefy: true,
                ..Actions::default()
            }
        );
        assert_eq!(
            Actions::parse("qDe"),
            Actions {
                quotes: true,
                dashes: 2,
                ellipses: true,
                ..Actions::default()
            }
        );
        assert_eq!(Actions::parse("Bb").backticks, 2);
        assert_eq!(Actions::parse("bB").backticks, 2);
        assert_eq!(Actions::parse("b").backticks, 1);
        assert_eq!(Actions::parse("iDd").dashes, 3);
        assert_eq!(Actions::parse("dD").dashes, 2);
        assert_eq!(
            Actions::parse("qw"),
            Actions {
                quotes: true,
                convert_quot: true,
                ..Actions::default()
            }
        );
        assert_eq!(
            Actions::parse("-1q"),
            Actions {
                quotes: true,
                ..Actions::default()
            }
        );
        for nothing in ["0", "", "x", "12"] {
            assert_eq!(Actions::parse(nothing), Actions::default(), "{nothing:?}");
        }
    }
}
