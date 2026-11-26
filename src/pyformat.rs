//! Python's string formatting and its reading of numbers, as CPython
//! 3.12.2 runs them, for what Sphinx 9.1.0 and docutils 0.22.4 hand them:
//!
//! - `str.format` with keyword arguments ([`format`]):
//!   `math_eqref_format.format(number=eqno)` at resolve time
//!   (`SP/sphinx/domains/math.py:126-131`), and a `numfig_format` entry or
//!   an explicit `:numref:` title, `title.format(name=figname,
//!   number=fignum)` or `title.format(number=fignum)`
//!   (`SP/sphinx/domains/std/__init__.py:1132-1159`);
//! - `str.format_map` ([`format_map`]): `manpages_url.format_map(info)` at
//!   parse time (`SP/sphinx/roles.py:531-551`), whose `section` is `None`
//!   when the target names none;
//! - `%` with one `str` argument ([`percent`]): `title % fignum`, for a
//!   title that holds neither `{name}` nor `number`;
//! - `int()` and `float()` over a `str` ([`int`], [`float`]): directive
//!   options and the PEP and RFC roles (docutils), and `SOURCE_DATE_EPOCH`;
//! - `chr()` of a code `int()` reads in base 10 or 16 ([`chr`]): docutils'
//!   `unicode_code`, for the `unicode` directive and the options that take
//!   a character;
//! - `repr()` of a `float` ([`float_repr`]): a length docutils reads with
//!   `float()` (`get_measure`), and a float constant `ast.unparse` writes
//!   (the Python domain's signatures);
//! - `str()` of a number a `conf.py` literal holds ([`number_str`]): what
//!   the `.buildinfo` config hash hashes (`stable_hash`), and `%r` in the
//!   errors of `intersphinx_mapping`.
//!
//! Each raises what CPython raises, with CPython's message, which Sphinx's
//! crash report prints (decision 13); each caller decides which of those
//! exceptions it catches ([`FormatError`]). Every branch is pinned against
//! CPython 3.12.2 by `tests/fixtures/pyformat_units.json`, which
//! `tools/gen_pyformat_fixture.py` records.
//!
//! Not modelled (T19b's divergence list): the value of an attribute that a
//! value has ([`FormatError::NotModelled`]).

use std::borrow::Cow;

use crate::rst::digits::decimal_digit_value;
use crate::rst::{LogLocation, SphinxCrash};
use crate::utils::py_repr_str;

/// A value a format reads: a `str`, or `None`, the `section` of a manual
/// page whose target names none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Value<'a> {
    Str(&'a str),
    None,
}

/// What Python raises formatting a `str` ([`format`], [`format_map`],
/// [`percent`]), by the kinds Sphinx's callers tell apart. A crash is
/// located by its caller.
#[derive(Debug)]
pub(crate) enum FormatError {
    /// `KeyError` for a field that names no argument, carrying the name:
    /// `_resolve_numref_xref` and `MathDomain.resolve_xref` catch it and
    /// warn; `Manpage.run` does not.
    KeyError(String),
    /// A `TypeError`: a string indexed by a name (`{number[x]}`), a format
    /// spec on `None` or an index into it, and `%`'s, for a mapping key, a
    /// `*`, a conversion a `str` does not take, a second conversion, or
    /// none.
    /// `_resolve_numref_xref` catches it; `MathDomain.resolve_xref` and
    /// `Manpage.run` do not, and Sphinx 9.1.0 crashes there (decision 13).
    TypeError(SphinxCrash),
    /// What none of Sphinx's callers catch, so that Sphinx 9.1.0 crashes
    /// (decision 13): a `ValueError` for a format the method cannot read, a
    /// conversion or a format spec it does not know; an `IndexError` for a
    /// positional field ([`format`] is given keywords only) or an index past
    /// a string's end; an `AttributeError` for an attribute the value
    /// lacks; a `MemoryError` for a width no allocation holds.
    Crash(SphinxCrash),
    /// The value of an attribute that the value has (`{number.upper}`, a
    /// bound method mostly, whose text names its address in memory), which
    /// Python goes on with and the port does not model (T19b's divergence
    /// list). Rendered without a spec, or after `!s`, `!r` or `!a` with a
    /// spec a `str` takes, it is read past, as Python reads past it, and
    /// the rest of the format raises what it raises; after a conversion it
    /// is a `str`, whose spec raises what `str.__format__` raises whatever
    /// the text. This is the outcome where the next step needs the value
    /// itself (a lookup on it, a spec built from its text, or rendering it
    /// with a spec and no conversion, whose outcome depends on its type),
    /// or where the format otherwise completes. Carries the whole text of
    /// the first field of the format that reaches such a value or builds
    /// its spec from one, which the callers that catch `KeyError` report
    /// as one.
    NotModelled(String),
}

/// A `ValueError`, which no caller catches.
fn value_error(message: impl Into<String>) -> FormatError {
    FormatError::Crash(SphinxCrash::python(
        "ValueError",
        message.into(),
        LogLocation::None,
    ))
}

/// A `TypeError`, which `_resolve_numref_xref` alone catches.
fn type_error(message: impl Into<String>) -> FormatError {
    FormatError::TypeError(SphinxCrash::python(
        "TypeError",
        message.into(),
        LogLocation::None,
    ))
}

/// An `IndexError`, which no caller catches.
fn index_error(message: impl Into<String>) -> FormatError {
    FormatError::Crash(SphinxCrash::python(
        "IndexError",
        message.into(),
        LogLocation::None,
    ))
}

/// `format.format(**keywords)`: Python's `str.format` given keyword
/// arguments alone, as Sphinx's math and standard domains call it.
pub(crate) fn format(format: &str, keywords: &[(&str, Value<'_>)]) -> Result<String, FormatError> {
    let chars: Vec<char> = format.chars().collect();
    let arguments = Arguments {
        keywords,
        mapping: false,
    };
    format_string(&chars, &arguments)
}

/// `format.format_map(mapping)`: Python's `str.format_map`, as `Manpage`
/// calls it, which takes no positional argument: a positional field is
/// the `ValueError` `Format string contains positional fields`, raised
/// where the field is looked up.
pub(crate) fn format_map(
    format: &str,
    mapping: &[(&str, Value<'_>)],
) -> Result<String, FormatError> {
    let chars: Vec<char> = format.chars().collect();
    let arguments = Arguments {
        keywords: mapping,
        mapping: true,
    };
    format_string(&chars, &arguments)
}

/// CPython's `do_string_format`: [`build_string`] from the recursion
/// limit, its text, or [`FormatError::NotModelled`] for a format whose
/// text the port does not know.
fn format_string(chars: &[char], arguments: &Arguments<'_, '_>) -> Result<String, FormatError> {
    match build_string(chars, arguments, MAX_STRING_RECURSION)? {
        Built::Text(text) => Ok(text),
        Built::NotModelled(field) => Err(not_modelled(field)),
    }
}

/// The depth `do_string_format` starts `build_string` at: a format spec
/// may hold fields, and theirs may not.
const MAX_STRING_RECURSION: u32 = 2;

/// What [`build_string`] makes of a format it reads to its end without
/// an exception.
enum Built<'f> {
    Text(String),
    /// The whole text of the first field that renders a value the port
    /// does not model ([`FormatError::NotModelled`]), without which the
    /// format's text is not known.
    NotModelled(&'f [char]),
}

/// What a format looks a field's first part up in.
struct Arguments<'k, 'v> {
    keywords: &'k [(&'v str, Value<'v>)],
    /// `format_map`'s mapping, where `format` has an empty tuple of
    /// positional arguments.
    mapping: bool,
}

/// CPython's `build_string` and `do_markup` (`Objects/stringlib/
/// unicode_format.h`) over `chars`, `depth` levels from the recursion
/// limit: the format read from the left (`MarkupIterator`), where `{{`
/// and `}}` are a literal brace, and a single `}`, or a `{` that ends the
/// format, is a `ValueError`; each field parsed ([`parse_field`]) and
/// filled ([`output_field`]) before the next is read. A field that renders
/// a value the port does not model, without a spec or converted with a
/// spec a `str` takes, is read past, as Python reads past it, so that the
/// rest of the format raises what it raises; the first such field names
/// the format's [`FormatError::NotModelled`], whether the format completes
/// ([`Built::NotModelled`]) or a later field stops it.
fn build_string<'f>(
    chars: &'f [char],
    arguments: &Arguments<'_, '_>,
    depth: u32,
) -> Result<Built<'f>, FormatError> {
    if depth == 0 {
        return Err(value_error("Max string recursion exceeded"));
    }
    let mut out = String::with_capacity(chars.len());
    let mut unmodelled = None;
    let mut at = 0;
    while let Some(&c) = chars.get(at) {
        at += 1;
        if c != '{' && c != '}' {
            out.push(c);
            continue;
        }
        if chars.get(at) == Some(&c) {
            out.push(c);
            at += 1;
            continue;
        }
        if c == '}' {
            return Err(value_error("Single '}' encountered in format string"));
        }
        if at == chars.len() {
            return Err(value_error("Single '{' encountered in format string"));
        }
        let field = parse_field(chars, &mut at)?;
        match output_field(&field, arguments, depth) {
            Ok(Some(text)) => out.push_str(&text),
            Ok(None) => {
                unmodelled.get_or_insert(field.text);
            }
            Err(FormatError::NotModelled(_)) => {
                return Err(not_modelled(unmodelled.unwrap_or(field.text)))
            }
            Err(error) => return Err(error),
        }
    }
    Ok(match unmodelled {
        Some(field) => Built::NotModelled(field),
        None => Built::Text(out),
    })
}

/// CPython's `output_markup`, one field of [`build_string`] at `depth`:
/// looked up ([`get_field_object`]), converted ([`convert`]), its spec
/// expanded with the same arguments when it holds a field, then rendered
/// ([`render_field`]). `None` for a value the port does not model,
/// rendered without a spec, or converted and rendered with a spec a `str`
/// takes, whose text Python writes and the port does not know. A spec
/// built from such a value is [`FormatError::NotModelled`].
fn output_field(
    field: &FormatField<'_>,
    arguments: &Arguments<'_, '_>,
    depth: u32,
) -> Result<Option<String>, FormatError> {
    let object = convert(get_field_object(field, arguments)?, field.conversion)?;
    let spec: Cow<'_, [char]> = if field.spec_needs_expanding {
        match build_string(field.spec, arguments, depth - 1)? {
            Built::Text(spec) => Cow::Owned(spec.chars().collect()),
            Built::NotModelled(_) => return Err(not_modelled(field.text)),
        }
    } else {
        Cow::Borrowed(field.spec)
    };
    render_field(object, &spec, field)
}

/// One `{field}` of a format, as CPython's `parse_field` splits it.
struct FormatField<'f> {
    /// The field name, up to its `!` or `:`.
    name: &'f [char],
    /// Everything between the braces, which a [`FormatError::NotModelled`]
    /// carries.
    text: &'f [char],
    /// The character after a `!`.
    conversion: Option<char>,
    /// The format spec after a `:`, up to the `}` that balances its braces.
    spec: &'f [char],
    /// Whether the spec holds a `{`: a format itself, expanded with the
    /// same arguments before it is applied.
    spec_needs_expanding: bool,
}

/// CPython's `parse_field`, from just after a field's `{` at `*at` to
/// just after its closing `}`: a name that ends at the first `}`, `:` or
/// `!` outside a `[...]` (a `{` in it is a `ValueError`); after a `!`, one
/// conversion character, then `}` or `:`; after a `:`, a format spec up to
/// the `}` that balances its braces. A field that does not close is a
/// `ValueError`.
fn parse_field<'f>(chars: &'f [char], at: &mut usize) -> Result<FormatField<'f>, FormatError> {
    let start = *at;
    let mut terminator = None;
    while let Some(&c) = chars.get(*at) {
        *at += 1;
        match c {
            '{' => return Err(value_error("unexpected '{' in field name")),
            '[' => {
                while chars.get(*at).is_some_and(|&c| c != ']') {
                    *at += 1;
                }
            }
            '}' | ':' | '!' => {
                terminator = Some(c);
                break;
            }
            _ => {}
        }
    }
    let Some(terminator) = terminator else {
        return Err(value_error("expected '}' before end of string"));
    };
    let name = &chars[start..*at - 1];
    let field = |end: usize, conversion, spec, spec_needs_expanding| FormatField {
        name,
        text: &chars[start..end],
        conversion,
        spec,
        spec_needs_expanding,
    };
    if terminator == '}' {
        return Ok(field(*at - 1, None, &[], false));
    }
    let mut conversion = None;
    if terminator == '!' {
        let Some(&c) = chars.get(*at) else {
            return Err(value_error(
                "end of string while looking for conversion specifier",
            ));
        };
        conversion = Some(c);
        *at += 1;
        if let Some(&next) = chars.get(*at) {
            *at += 1;
            if next == '}' {
                return Ok(field(*at - 1, conversion, &[], false));
            }
            if next != ':' {
                return Err(value_error("expected ':' after conversion specifier"));
            }
        }
    }
    let spec_start = *at;
    let mut depth = 1;
    let mut spec_needs_expanding = false;
    while let Some(&c) = chars.get(*at) {
        *at += 1;
        match c {
            '{' => {
                spec_needs_expanding = true;
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let spec = &chars[spec_start..*at - 1];
                    return Ok(field(*at - 1, conversion, spec, spec_needs_expanding));
                }
            }
            _ => {}
        }
    }
    Err(value_error("unmatched '{' in format spec"))
}

/// What a field's lookups reach.
enum Object {
    Str(String),
    None,
    /// The value of an attribute that the value before it has
    /// ([`FormatError::NotModelled`]).
    NotModelled,
}

/// The [`FormatError::NotModelled`] of a field whose whole text is
/// `field`.
fn not_modelled(field: &[char]) -> FormatError {
    FormatError::NotModelled(field.iter().collect())
}

/// CPython's `get_field_object`: the name's first part, up to a `.` or
/// `[` (`field_name_split`), is a positional index when it is empty or all
/// decimal digits, of any script ([`format_integer`]): an `IndexError`
/// for [`format`], which is given no positional argument, and a
/// `ValueError` for [`format_map`], which takes none; otherwise a keyword,
/// or a `KeyError`. The rest of the name is looked up on what the keyword
/// gives, one `.attr` or `[item]` at a time, each before the next is read
/// (`FieldNameIterator`): an empty one, or anything but `.` or `[` after a
/// `]`, is a `ValueError`; an item of decimal digits, of any script, is an
/// index, and any other item a name ([`get_item`]); an attribute is looked
/// up with `getattr` ([`get_attribute`]).
fn get_field_object(
    field: &FormatField<'_>,
    arguments: &Arguments<'_, '_>,
) -> Result<Object, FormatError> {
    let name = field.name;
    let first_len = name
        .iter()
        .position(|&c| c == '.' || c == '[')
        .unwrap_or(name.len());
    let index = format_integer(&name[..first_len])?;
    if first_len == 0 || index.is_some() {
        if arguments.mapping {
            return Err(value_error("Format string contains positional fields"));
        }
        // An empty name is the next automatic index, which is always 0
        // here: the first positional field raises.
        return Err(index_error(format!(
            "Replacement index {} out of range for positional args tuple",
            index.unwrap_or(0)
        )));
    }
    let first: String = name[..first_len].iter().collect();
    let mut object = match arguments.keywords.iter().find(|(key, _)| *key == first) {
        Some((_, Value::Str(text))) => Object::Str(text.to_string()),
        Some((_, Value::None)) => Object::None,
        None => return Err(FormatError::KeyError(first)),
    };
    let mut at = first_len;
    while let Some(&c) = name.get(at) {
        at += 1;
        let start = at;
        let attribute = match c {
            '.' => {
                while name.get(at).is_some_and(|&c| c != '.' && c != '[') {
                    at += 1;
                }
                true
            }
            '[' => {
                while name.get(at).is_some_and(|&c| c != ']') {
                    at += 1;
                }
                if at == name.len() {
                    // Not reached through `str.format`, whose field name
                    // closes every `[` ([`parse_field`]).
                    return Err(value_error("Missing ']' in format string"));
                }
                at += 1;
                false
            }
            _ => {
                return Err(value_error(
                    "Only '.' or '[' may follow ']' in format field specifier",
                ))
            }
        };
        let part = &name[start..if attribute { at } else { at - 1 }];
        let index = if attribute {
            None
        } else {
            format_integer(part)?
        };
        if part.is_empty() {
            return Err(value_error("Empty attribute in format string"));
        }
        object = if attribute {
            get_attribute(&object, &part.iter().collect::<String>(), field)?
        } else {
            get_item(object, index, field)?
        };
    }
    Ok(object)
}

/// CPython's `get_integer` in `Objects/stringlib/unicode_format.h`, over
/// a part of a field name: its value when it is all decimal digits, of any
/// script, as `Py_UNICODE_TODECIMAL` reads them ([`decimal_digit_value`]:
/// `{٣}` is the positional field 3), read from the left, so that a value
/// past `isize::MAX` is a `ValueError` even before a character that is no
/// digit; `None` for an empty part or one with a character that is no
/// digit.
fn format_integer(part: &[char]) -> Result<Option<i64>, FormatError> {
    if part.is_empty() {
        return Ok(None);
    }
    let mut value: i64 = 0;
    for &c in part {
        let Some(digit) = decimal_digit_value(c) else {
            return Ok(None);
        };
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add(i64::from(digit)))
            .ok_or_else(|| value_error("Too many decimal digits in format string"))?;
    }
    Ok(Some(value))
}

/// `getattr(object, name)` in `field`: an attribute the value has is a
/// value the port does not model, and one it lacks the `AttributeError` of
/// `PyObject_GenericGetAttr`, as the last line of the traceback Sphinx's
/// crash report prints it, which adds the suggestion `TracebackException`
/// makes for an `AttributeError` that carries its name and object
/// ([`attribute_suggestion`]). On a value the port does not model, the
/// lookup is not modelled either ([`FormatError::NotModelled`]).
fn get_attribute(
    object: &Object,
    name: &str,
    field: &FormatField<'_>,
) -> Result<Object, FormatError> {
    let (type_name, attributes): (&str, &[&str]) = match object {
        Object::Str(_) => ("str", &STR_ATTRIBUTES),
        Object::None => ("NoneType", &NONE_ATTRIBUTES),
        Object::NotModelled => return Err(not_modelled(field.text)),
    };
    if attributes.contains(&name) {
        return Ok(Object::NotModelled);
    }
    let mut message = format!("'{type_name}' object has no attribute '{name}'");
    if let Some(suggestion) = attribute_suggestion(attributes, name) {
        message.push_str(&format!(". Did you mean: '{suggestion}'?"));
    }
    Err(FormatError::Crash(SphinxCrash::python(
        "AttributeError",
        message,
        LogLocation::None,
    )))
}

/// `object[index]` for an item of digits, `object[name]` for any other
/// (`index` is `None`), in `field`: a string's character at that index
/// (`PySequence_GetItem`, the index never negative here), or an
/// `IndexError` past its end, and a `TypeError` for a name; `None` takes
/// neither. On a value the port does not model, the lookup is not
/// modelled either ([`FormatError::NotModelled`]).
fn get_item(
    object: Object,
    index: Option<i64>,
    field: &FormatField<'_>,
) -> Result<Object, FormatError> {
    match (object, index) {
        (Object::Str(text), Some(index)) => usize::try_from(index)
            .ok()
            .and_then(|index| text.chars().nth(index))
            .map(|c| Object::Str(c.to_string()))
            .ok_or_else(|| index_error("string index out of range")),
        (Object::Str(_), None) => Err(type_error("string indices must be integers, not 'str'")),
        (Object::None, _) => Err(type_error("'NoneType' object is not subscriptable")),
        (Object::NotModelled, _) => Err(not_modelled(field.text)),
    }
}

/// `dir('')` on CPython 3.12, sorted as `dir` sorts it: every attribute a
/// `str` has, the ones `getattr` finds.
const STR_ATTRIBUTES: [&str; 81] = [
    "__add__",
    "__class__",
    "__contains__",
    "__delattr__",
    "__dir__",
    "__doc__",
    "__eq__",
    "__format__",
    "__ge__",
    "__getattribute__",
    "__getitem__",
    "__getnewargs__",
    "__getstate__",
    "__gt__",
    "__hash__",
    "__init__",
    "__init_subclass__",
    "__iter__",
    "__le__",
    "__len__",
    "__lt__",
    "__mod__",
    "__mul__",
    "__ne__",
    "__new__",
    "__reduce__",
    "__reduce_ex__",
    "__repr__",
    "__rmod__",
    "__rmul__",
    "__setattr__",
    "__sizeof__",
    "__str__",
    "__subclasshook__",
    "capitalize",
    "casefold",
    "center",
    "count",
    "encode",
    "endswith",
    "expandtabs",
    "find",
    "format",
    "format_map",
    "index",
    "isalnum",
    "isalpha",
    "isascii",
    "isdecimal",
    "isdigit",
    "isidentifier",
    "islower",
    "isnumeric",
    "isprintable",
    "isspace",
    "istitle",
    "isupper",
    "join",
    "ljust",
    "lower",
    "lstrip",
    "maketrans",
    "partition",
    "removeprefix",
    "removesuffix",
    "replace",
    "rfind",
    "rindex",
    "rjust",
    "rpartition",
    "rsplit",
    "rstrip",
    "split",
    "splitlines",
    "startswith",
    "strip",
    "swapcase",
    "title",
    "translate",
    "upper",
    "zfill",
];

/// `dir(None)` on CPython 3.12, sorted as `dir` sorts it: every attribute
/// `None` has, the ones `getattr` finds.
const NONE_ATTRIBUTES: [&str; 25] = [
    "__bool__",
    "__class__",
    "__delattr__",
    "__dir__",
    "__doc__",
    "__eq__",
    "__format__",
    "__ge__",
    "__getattribute__",
    "__getstate__",
    "__gt__",
    "__hash__",
    "__init__",
    "__init_subclass__",
    "__le__",
    "__lt__",
    "__ne__",
    "__new__",
    "__reduce__",
    "__reduce_ex__",
    "__repr__",
    "__setattr__",
    "__sizeof__",
    "__str__",
    "__subclasshook__",
];

/// `_compute_suggestion_error` (CPython 3.12 `Lib/traceback.py`) over
/// `attributes`, the value's `dir()` ([`STR_ATTRIBUTES`],
/// [`NONE_ATTRIBUTES`]): of the names it has, in `dir` order, the first
/// closest to `wrong` by [`suggestion_distance`], provided no more than a
/// third of the characters involved change; none for a `wrong` longer
/// than 40 characters.
fn attribute_suggestion(attributes: &[&'static str], wrong: &str) -> Option<&'static str> {
    let wrong: Vec<char> = wrong.chars().collect();
    // (`dir(obj)` past 750 names gives none: `str` has 81, `None` 25.)
    if wrong.len() > SUGGESTION_MAX_STRING_SIZE {
        return None;
    }
    let mut best_distance = wrong.len();
    let mut suggestion = None;
    // (`wrong` itself is never among them: `getattr` did not find it.)
    for &possible in attributes {
        let possible_chars: Vec<char> = possible.chars().collect();
        // No more than 1/3 of the involved characters should need
        // changing, and a match must beat the best one so far.
        let max_distance = ((possible_chars.len() + wrong.len() + 3) * SUGGESTION_MOVE_COST / 6)
            .min(best_distance.saturating_sub(1));
        let distance = suggestion_distance(&wrong, &possible_chars, max_distance);
        if distance > max_distance {
            continue;
        }
        if suggestion.is_none() || distance < best_distance {
            suggestion = Some(possible);
            best_distance = distance;
        }
    }
    suggestion
}

/// The bound and the costs of Python's suggestions (CPython 3.12
/// `Lib/traceback.py`: `_MAX_STRING_SIZE`, `_MOVE_COST`, `_CASE_COST`).
const SUGGESTION_MAX_STRING_SIZE: usize = 40;
const SUGGESTION_MOVE_COST: usize = 2;
const SUGGESTION_CASE_COST: usize = 1;

/// `_levenshtein_distance` (CPython 3.12 `Lib/traceback.py`, after
/// `Python/suggestions.c`): the cost of turning `a` into `b`, where a
/// character that only differs in case costs 1 and any other insertion,
/// deletion or substitution 2, once their common prefix and suffix are
/// trimmed; `max_cost + 1` as soon as the cost must exceed `max_cost`.
fn suggestion_distance(a: &[char], b: &[char], max_cost: usize) -> usize {
    if a == b {
        return 0;
    }
    // Trim away common affixes.
    let pre = a.iter().zip(b).take_while(|(a, b)| a == b).count();
    let (a, b) = (&a[pre..], &b[pre..]);
    let post = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (a, b) = (&a[..a.len() - post], &b[..b.len() - post]);
    if a.is_empty() || b.is_empty() {
        return SUGGESTION_MOVE_COST * (a.len() + b.len());
    }
    if a.len() > SUGGESTION_MAX_STRING_SIZE || b.len() > SUGGESTION_MAX_STRING_SIZE {
        return max_cost + 1;
    }
    // Prefer the shorter buffer.
    let (a, b) = if b.len() < a.len() { (b, a) } else { (a, b) };
    // Quick fail when a match is impossible.
    if (b.len() - a.len()) * SUGGESTION_MOVE_COST > max_cost {
        return max_cost + 1;
    }
    // One row of the matrix, updated in place.
    let mut row: Vec<usize> = (1..=a.len())
        .map(|index| index * SUGGESTION_MOVE_COST)
        .collect();
    let mut result = 0;
    for (bindex, &bchar) in b.iter().enumerate() {
        let mut distance = bindex * SUGGESTION_MOVE_COST;
        result = distance;
        let mut minimum = usize::MAX;
        for (index, &achar) in a.iter().enumerate() {
            // `_substitution_cost`.
            let substitute = distance
                + if bchar == achar {
                    0
                } else if bchar.to_lowercase().eq(achar.to_lowercase()) {
                    SUGGESTION_CASE_COST
                } else {
                    SUGGESTION_MOVE_COST
                };
            distance = row[index];
            let insert_delete = result.min(distance) + SUGGESTION_MOVE_COST;
            result = insert_delete.min(substitute);
            row[index] = result;
            minimum = minimum.min(result);
        }
        if minimum > max_cost {
            // Everything in this row is too big, so bail early.
            return max_cost + 1;
        }
    }
    result
}

/// CPython's `do_conversion`: `!s`, `!r` and `!a` give `str()`, `repr()`
/// ([`py_repr_str`]) and `ascii()` ([`ascii`]) of the value, `'None'` for
/// `None`; any other character is a `ValueError` that names it as itself
/// from `!` to `~`, and by its code point in hex otherwise. A value the
/// port does not model stays one, a `str` whose text it does not know
/// once converted, which [`render_field`] tells by the field's conversion.
fn convert(object: Object, conversion: Option<char>) -> Result<Object, FormatError> {
    let Some(conversion) = conversion else {
        return Ok(object);
    };
    if !matches!(conversion, 's' | 'r' | 'a') {
        return Err(value_error(if ('!'..='~').contains(&conversion) {
            format!("Unknown conversion specifier {conversion}")
        } else {
            format!(
                "Unknown conversion specifier \\x{:x}",
                u32::from(conversion)
            )
        }));
    }
    Ok(match object {
        Object::Str(text) => Object::Str(match conversion {
            's' => text,
            'r' => py_repr_str(&text),
            _ => ascii(&text),
        }),
        Object::None => Object::Str("None".to_string()),
        Object::NotModelled => Object::NotModelled,
    })
}

/// CPython's `render_field`: a `str` by `str.__format__` ([`format_str`]),
/// and `None` by `object.__format__`, which gives `'None'` for an empty
/// spec and raises `TypeError` for any other. A value the port does not
/// model gives `None` for an empty spec, its text, which the port does not
/// know. After the field's `!s`, `!r` or `!a` it is a `str` whose text the
/// port does not know: its spec raises what `str.__format__` raises
/// whatever the text ([`check_unknown_str`]), and otherwise gives `None`
/// too. With any other spec it is [`FormatError::NotModelled`], whose
/// outcome depends on the value's type.
fn render_field(
    object: Object,
    spec: &[char],
    field: &FormatField<'_>,
) -> Result<Option<String>, FormatError> {
    match object {
        Object::Str(text) => format_str(&text, spec).map(Some),
        Object::None if spec.is_empty() => Ok(Some("None".to_string())),
        Object::None => Err(type_error(
            "unsupported format string passed to NoneType.__format__",
        )),
        Object::NotModelled if spec.is_empty() => Ok(None),
        Object::NotModelled if field.conversion.is_some() => check_unknown_str(spec).map(|()| None),
        Object::NotModelled => Err(not_modelled(field.text)),
    }
}

/// `str.__format__(spec)` (CPython's `_PyUnicode_FormatAdvancedWriter`):
/// the text itself for an empty spec; otherwise the spec as a `str` takes
/// it ([`str_spec`]), then the precision keeps that many code points, and
/// the fill pads them to the width, its odd one on the right when centred
/// (`calc_padding`).
fn format_str(text: &str, spec: &[char]) -> Result<String, FormatError> {
    if spec.is_empty() {
        return Ok(text.to_string());
    }
    let spec = str_spec(spec)?;
    let len = text.chars().count();
    let kept = spec.precision.map_or(len, |precision| precision.min(len));
    let total = spec.width.map_or(kept, |width| width.max(kept));
    let left = match spec.align {
        '>' => total - kept,
        '^' => (total - kept) / 2,
        _ => 0,
    };
    padded(text, kept, spec.fill, left, total - kept - left)
}

/// A non-empty spec as `str.__format__` takes it, which raises the same
/// whatever the text: the spec as `s` and `<` default it
/// ([`StrSpec::parse`]), any type but `s` a `ValueError`, then
/// `format_string_internal`'s checks, where a sign, `z`, `#` or the `=`
/// alignment is a `ValueError`.
fn str_spec(spec: &[char]) -> Result<StrSpec, FormatError> {
    let spec = StrSpec::parse(spec)?;
    if spec.kind != 's' {
        return Err(value_error(format!(
            "Unknown format code {} for object of type 'str'",
            quoted_type(spec.kind)
        )));
    }
    match spec.sign {
        Some(' ') => return Err(value_error("Space not allowed in string format specifier")),
        Some(_) => return Err(value_error("Sign not allowed in string format specifier")),
        None => {}
    }
    if spec.no_negative_zero {
        return Err(value_error(
            "Negative zero coercion (z) not allowed in string format specifier",
        ));
    }
    if spec.alternate {
        return Err(value_error(
            "Alternate form (#) not allowed in string format specifier",
        ));
    }
    if spec.align == '=' {
        return Err(value_error(
            "'=' alignment not allowed in string format specifier",
        ));
    }
    Ok(spec)
}

/// `str.__format__(spec)` over a `str` whose text the port does not know:
/// `str()`, `repr()` or `ascii()` of a value it does not model. What
/// [`format_str`] raises whatever the text: the spec's errors
/// ([`str_spec`]), and the `MemoryError` of a width no allocation holds,
/// which the text, far shorter than such a width, is padded to; room for
/// the width's fill alone is tried, as [`padded`] tries it for an empty
/// text.
fn check_unknown_str(spec: &[char]) -> Result<(), FormatError> {
    if spec.is_empty() {
        return Ok(());
    }
    let spec = str_spec(spec)?;
    if let Some(width) = spec.width {
        reserved(width.checked_mul(spec.fill.len_utf8()))?;
    }
    Ok(())
}

/// The first `kept` characters of `text` between `left` and `right`
/// copies of `fill`, in the room [`reserved`] makes for them, or its
/// `MemoryError`.
fn padded(
    text: &str,
    kept: usize,
    fill: char,
    left: usize,
    right: usize,
) -> Result<String, FormatError> {
    let kept_bytes = text
        .char_indices()
        .nth(kept)
        .map_or(text.len(), |(at, _)| at);
    let mut out = reserved(
        left.checked_add(right)
            .and_then(|fills| fills.checked_mul(fill.len_utf8()))
            .and_then(|fills| fills.checked_add(kept_bytes)),
    )?;
    out.extend(std::iter::repeat_n(fill, left));
    out.push_str(&text[..kept_bytes]);
    out.extend(std::iter::repeat_n(fill, right));
    Ok(out)
}

/// An empty `String` that holds `bytes` (`None`: more than `usize`
/// counts), or the `MemoryError` CPython's writer raises when no
/// allocation holds them (`_PyUnicodeWriter_Prepare`).
fn reserved(bytes: Option<usize>) -> Result<String, FormatError> {
    let mut out = String::new();
    if bytes.is_none_or(|bytes| out.try_reserve_exact(bytes).is_err()) {
        return Err(FormatError::Crash(SphinxCrash::python(
            "MemoryError",
            String::new(),
            LogLocation::None,
        )));
    }
    Ok(out)
}

/// A format spec as `str.__format__` reads it.
struct StrSpec {
    fill: char,
    align: char,
    sign: Option<char>,
    no_negative_zero: bool,
    alternate: bool,
    width: Option<usize>,
    precision: Option<usize>,
    kind: char,
}

impl StrSpec {
    /// CPython's `parse_internal_render_format_spec` (`Python/
    /// formatter_unicode.c`) over `spec`, with `s` as the default type and
    /// `<` as the default alignment: a fill character before an alignment
    /// (`<`, `>`, `=`, `^`), a sign (`+`, `-`, space), `z`, `#`, a `0` that
    /// makes `0` the fill when none is given (the alignment stays `<`), the
    /// width, `,` or `_` (not both: a `ValueError`), a `.` and the
    /// precision (a `ValueError` without one), widths and precisions in
    /// decimal digits of any script ([`spec_integer`]); then at most one
    /// character, the type, or the whole spec is a `ValueError`; and a
    /// separator the type does not take is one too.
    fn parse(spec: &[char]) -> Result<StrSpec, FormatError> {
        let is_align = |c: char| matches!(c, '<' | '>' | '=' | '^');
        let mut parsed = StrSpec {
            fill: ' ',
            align: '<',
            sign: None,
            no_negative_zero: false,
            alternate: false,
            width: None,
            precision: None,
            kind: 's',
        };
        let mut at = 0;
        let mut fill_specified = false;
        if spec.len() >= 2 && is_align(spec[1]) {
            parsed.fill = spec[0];
            parsed.align = spec[1];
            fill_specified = true;
            at = 2;
        } else if spec.first().copied().is_some_and(is_align) {
            parsed.align = spec[0];
            at = 1;
        }
        if let Some(&sign) = spec.get(at).filter(|&&c| matches!(c, ' ' | '+' | '-')) {
            parsed.sign = Some(sign);
            at += 1;
        }
        if spec.get(at) == Some(&'z') {
            parsed.no_negative_zero = true;
            at += 1;
        }
        if spec.get(at) == Some(&'#') {
            parsed.alternate = true;
            at += 1;
        }
        if !fill_specified && spec.get(at) == Some(&'0') {
            parsed.fill = '0';
            at += 1;
        }
        parsed.width = spec_integer(spec, &mut at)?;
        let mut separator = None;
        if spec.get(at) == Some(&',') {
            separator = Some(',');
            at += 1;
        }
        if spec.get(at) == Some(&'_') {
            if separator.is_some() {
                return Err(value_error("Cannot specify both ',' and '_'."));
            }
            separator = Some('_');
            at += 1;
        }
        if spec.get(at) == Some(&',') && separator == Some('_') {
            return Err(value_error("Cannot specify both ',' and '_'."));
        }
        if spec.get(at) == Some(&'.') {
            at += 1;
            parsed.precision = spec_integer(spec, &mut at)?;
            if parsed.precision.is_none() {
                return Err(value_error("Format specifier missing precision"));
            }
        }
        match &spec[at..] {
            [] => {}
            [kind] => parsed.kind = *kind,
            _ => {
                return Err(value_error(format!(
                    "Invalid format specifier '{}' for object of type 'str'",
                    spec.iter().collect::<String>()
                )))
            }
        }
        if let Some(separator) = separator {
            let takes_it = match parsed.kind {
                'd' | 'e' | 'f' | 'g' | 'E' | 'G' | '%' | 'F' | '\0' => true,
                'b' | 'o' | 'x' | 'X' => separator == '_',
                _ => false,
            };
            if !takes_it {
                return Err(value_error(format!(
                    "Cannot specify '{separator}' with {}.",
                    quoted_type(parsed.kind)
                )));
            }
        }
        Ok(parsed)
    }
}

/// CPython's `get_integer` in `Python/formatter_unicode.c`: the decimal
/// digits, of any script, from `*at`, as a width or a precision, `None`
/// when there are none; a value past `isize::MAX` is a `ValueError`.
fn spec_integer(spec: &[char], at: &mut usize) -> Result<Option<usize>, FormatError> {
    let start = *at;
    let mut value: i64 = 0;
    while let Some(digit) = spec.get(*at).and_then(|&c| decimal_digit_value(c)) {
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add(i64::from(digit)))
            .ok_or_else(|| value_error("Too many decimal digits in format string"))?;
        *at += 1;
    }
    Ok((*at > start).then(|| usize::try_from(value).unwrap_or(usize::MAX)))
}

/// A presentation type as CPython's messages quote it
/// (`unknown_presentation_type`, `invalid_thousands_separator_type`): as
/// itself from `!` to DEL, and by its code point in hex otherwise.
fn quoted_type(kind: char) -> String {
    if ('!'..='\x7f').contains(&kind) {
        format!("'{kind}'")
    } else {
        format!("'\\x{:x}'", u32::from(kind))
    }
}

/// Python's `ascii()` of a `str` (`PyObject_ASCII`): its `repr()`
/// ([`py_repr_str`]), each character past ASCII then escaped as `\xNN`,
/// `\uNNNN` or `\UNNNNNNNN` (`backslashreplace`). Every character
/// `py_repr_str` leaves unescaped where `repr()` escapes it (categories
/// Cf, Co and Cn) is past ASCII, so `ascii()` is exact.
fn ascii(text: &str) -> String {
    let mut out = String::new();
    for c in py_repr_str(text).chars() {
        let code = u32::from(c);
        if code < 0x80 {
            out.push(c);
        } else if code <= 0xff {
            out.push_str(&format!("\\x{code:02x}"));
        } else if code <= 0xffff {
            out.push_str(&format!("\\u{code:04x}"));
        } else {
            out.push_str(&format!("\\U{code:08x}"));
        }
    }
    out
}

/// `format % arg` for one `str` argument, neither a tuple nor a mapping
/// (CPython's `PyUnicode_Format` with `arglen` -1): `%%` is a `%`, and the
/// argument fills the one conversion ([`percent_conversion`]); a format
/// that never takes it is `TypeError: not all arguments converted during
/// string formatting`.
pub(crate) fn percent(format: &str, arg: &str) -> Result<String, FormatError> {
    let chars: Vec<char> = format.chars().collect();
    let mut out = String::with_capacity(format.len());
    let mut taken = false;
    let mut at = 0;
    while let Some(&c) = chars.get(at) {
        at += 1;
        if c != '%' {
            out.push(c);
            continue;
        }
        if chars.get(at) == Some(&'%') {
            out.push('%');
            at += 1;
            continue;
        }
        out.push_str(&percent_conversion(&chars, &mut at, arg, &mut taken)?);
    }
    if !taken {
        return Err(type_error(
            "not all arguments converted during string formatting",
        ));
    }
    Ok(out)
}

/// One conversion of [`percent`], from just after its `%` at `*at` to
/// just after its conversion character (CPython's `unicode_format_arg`,
/// whose `fmtcnt` is `remaining`, and where reading past the end reads the
/// string's NUL terminator):
///
/// - `(` is `TypeError: format requires a mapping`;
/// - the flags `-`, `+`, space, `#` and `0`, of which only `-` changes a
///   string: it pads on the right;
/// - a width and a `.` and a precision, in ASCII digits (`width too big`
///   past `isize::MAX`, `precision too big` past `INT_MAX`: `ValueError`s),
///   or `*`, which takes the argument, a `str`: `TypeError: * wants int`;
/// - one of `h`, `l` and `L`, skipped;
/// - a format that ends before its conversion character is `ValueError:
///   incomplete format`;
/// - then the argument is taken, before the character is read: a second
///   conversion is `TypeError: not enough arguments for format string`;
/// - `s`, `r` and `a` render `str()`, `repr()` and `ascii()` of it, cut to
///   the precision; `c` renders a one-character `str`, and is a
///   `TypeError` for any other; the numeric conversions are `TypeError`s;
///   any other character is a `ValueError` that names it, as itself from
///   `\x1f` to `~` and as `?` otherwise, with its code point and index;
/// - the text is padded with spaces to the width.
fn percent_conversion(
    chars: &[char],
    at: &mut usize,
    arg: &str,
    taken: &mut bool,
) -> Result<String, FormatError> {
    let read = |at: usize| chars.get(at).copied().unwrap_or('\0');
    let mut remaining = chars.len() as i64 - *at as i64;
    let mut c = read(*at);
    if c == '(' {
        return Err(type_error("format requires a mapping"));
    }
    let mut left_justify = false;
    loop {
        remaining -= 1;
        if remaining < 0 {
            break;
        }
        c = read(*at);
        *at += 1;
        match c {
            '-' => left_justify = true,
            '+' | ' ' | '#' | '0' => {}
            _ => break,
        }
    }
    let take = |taken: &mut bool| {
        if std::mem::replace(taken, true) {
            Err(type_error("not enough arguments for format string"))
        } else {
            Ok(())
        }
    };
    let mut width = None;
    if c == '*' {
        take(taken)?;
        return Err(type_error("* wants int"));
    }
    if let Some(digit) = c.to_digit(10) {
        let mut value = i64::from(digit);
        loop {
            remaining -= 1;
            if remaining < 0 {
                break;
            }
            c = read(*at);
            *at += 1;
            let Some(digit) = c.to_digit(10) else {
                break;
            };
            value = value
                .checked_mul(10)
                .and_then(|value| value.checked_add(i64::from(digit)))
                .ok_or_else(|| value_error("width too big"))?;
        }
        width = Some(usize::try_from(value).unwrap_or(usize::MAX));
    }
    let mut precision = None;
    if c == '.' {
        let mut value = 0;
        remaining -= 1;
        if remaining >= 0 {
            c = read(*at);
            *at += 1;
        }
        if c == '*' {
            take(taken)?;
            return Err(type_error("* wants int"));
        }
        if let Some(digit) = c.to_digit(10) {
            value = digit;
            loop {
                remaining -= 1;
                if remaining < 0 {
                    break;
                }
                c = read(*at);
                *at += 1;
                let Some(digit) = c.to_digit(10) else {
                    break;
                };
                value = value
                    .checked_mul(10)
                    .and_then(|value| value.checked_add(digit))
                    .filter(|&value| value <= i32::MAX as u32)
                    .ok_or_else(|| value_error("precision too big"))?;
            }
        }
        precision = Some(value as usize);
    }
    if remaining >= 0 && matches!(c, 'h' | 'l' | 'L') {
        remaining -= 1;
        if remaining >= 0 {
            c = read(*at);
            *at += 1;
        }
    }
    if remaining < 0 {
        return Err(value_error("incomplete format"));
    }
    take(taken)?;
    let text = match c {
        's' => arg.to_string(),
        'r' => py_repr_str(arg),
        'a' => ascii(arg),
        'c' => {
            let mut chars = arg.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c.to_string(),
                _ => return Err(type_error("%c requires int or char")),
            }
        }
        'i' | 'd' | 'u' => {
            return Err(type_error(format!(
                "%{c} format: a real number is required, not str"
            )))
        }
        'o' | 'x' | 'X' => {
            return Err(type_error(format!(
                "%{c} format: an integer is required, not str"
            )))
        }
        'e' | 'E' | 'f' | 'F' | 'g' | 'G' => {
            return Err(type_error("must be real number, not str"))
        }
        _ => {
            let shown = if ('\x1f'..='~').contains(&c) { c } else { '?' };
            return Err(value_error(format!(
                "unsupported format character '{shown}' ({:#x}) at index {}",
                u32::from(c),
                *at - 1
            )));
        }
    };
    let len = text.chars().count();
    let kept = match (c, precision) {
        ('s' | 'r' | 'a', Some(precision)) => precision.min(len),
        _ => len,
    };
    let pad = width.map_or(0, |width| width.saturating_sub(kept));
    if left_justify {
        padded(&text, kept, ' ', 0, pad)
    } else {
        padded(&text, kept, ' ', pad, 0)
    }
}

/// Python's `int(text)` for a `str`, in base 10 (CPython's
/// `PyLong_FromUnicodeObject`): the value, as its sign and its canonical
/// decimal digits at any precision (`-0` reads as 0), or the message of
/// the `ValueError` it raises. The text made ASCII ([`number_ascii`])
/// must be ASCII whitespace ([`ascii_space`]), a sign, digits that single
/// underscores may group (PEP 515), and ASCII whitespace, or it is an
/// `invalid literal`, named by its `repr()` cut to 200 characters
/// (`%.200R`); and once it is, more than [`INT_MAX_STR_DIGITS`] digits,
/// leading zeros among them, are past the limit. So the whitespace it
/// strips is Unicode's `White_Space`, not the C0 separators `\x1c`-`\x1f`
/// that `str.isspace` also admits (probed over every code point:
/// `tests/fixtures/pyformat_units.json`).
pub(crate) fn int(text: &str) -> Result<(bool, String), String> {
    let ascii = number_ascii(text);
    let unsigned = ascii.trim_start_matches(ascii_space);
    let (negative, unsigned) = match unsigned.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, unsigned.strip_prefix('+').unwrap_or(unsigned)),
    };
    let end = unsigned
        .find(|c: char| !c.is_ascii_digit() && c != '_')
        .unwrap_or(unsigned.len());
    let (number, rest) = unsigned.split_at(end);
    let reads = !number.is_empty()
        && number.split('_').all(|group| !group.is_empty())
        && rest.chars().all(ascii_space);
    if !reads {
        let literal: String = py_repr_str(text).chars().take(200).collect();
        return Err(format!("invalid literal for int() with base 10: {literal}"));
    }
    let digits: String = number.chars().filter(|&c| c != '_').collect();
    if digits.len() > INT_MAX_STR_DIGITS {
        return Err(format!(
            "Exceeds the limit ({INT_MAX_STR_DIGITS} digits) for integer string conversion: \
             value has {} digits; use sys.set_int_max_str_digits() to increase the limit",
            digits.len()
        ));
    }
    let canonical = digits.trim_start_matches('0');
    if canonical.is_empty() {
        return Ok((false, "0".to_string()));
    }
    Ok((negative, canonical.to_string()))
}

/// `sys.get_int_max_str_digits()` on CPython 3.12, which Sphinx and
/// docutils leave at its default: the most digits `int()` reads in base
/// 10.
const INT_MAX_STR_DIGITS: usize = 4300;

/// Python's `float(text)` for a `str` (CPython's `PyFloat_FromString`):
/// the text made ASCII ([`number_ascii`]), its single underscores dropped,
/// each between two digits (PEP 515), ASCII whitespace stripped around it
/// ([`ascii_space`]), then a sign and `inf`, `infinity` or `nan` in any
/// case, or a decimal literal of any number of digits; or the message of
/// the `ValueError` it raises, which names the text by its whole `repr()`.
pub(crate) fn float(text: &str) -> Result<f64, String> {
    let chars: Vec<char> = number_ascii(text).chars().collect();
    let grouped = chars.iter().enumerate().all(|(at, &c)| {
        c != '_'
            || (at > 0
                && chars[at - 1].is_ascii_digit()
                && chars.get(at + 1).is_some_and(char::is_ascii_digit))
    });
    grouped
        .then(|| {
            chars
                .into_iter()
                .filter(|&c| c != '_')
                .collect::<String>()
                .trim_matches(ascii_space)
                .parse()
                .ok()
        })
        .flatten()
        .ok_or_else(|| format!("could not convert string to float: {}", py_repr_str(text)))
}

/// What `chr()` raises for an `int` it makes no character of (CPython
/// 3.12's `builtin_chr`, which reads a C `int` with `PyLong_AsInt`, then
/// `PyUnicode_FromOrdinal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChrError {
    /// `OverflowError: Python int too large to convert to C int`: past
    /// `INT_MAX`, `2**31 - 1`.
    Overflow,
    /// `ValueError: chr() arg not in range(0x110000)`: from 0x110000 up to
    /// `INT_MAX`.
    NotInRange,
}

impl ChrError {
    /// The exception's message.
    pub(crate) fn message(self) -> &'static str {
        match self {
            ChrError::Overflow => "Python int too large to convert to C int",
            ChrError::NotInRange => "chr() arg not in range(0x110000)",
        }
    }
}

/// Python's `chr(i)` for a non-negative `int` written as `digits` in
/// `radix`: ASCII digits, as many as they come, leading zeros among them,
/// as [`int`] gives them in base 10, or as docutils' `unicode_pattern`
/// matches a hexadecimal code for `int(value, 16)`, which has no digit
/// limit. The code point, a surrogate among them, which Python makes a
/// `str` of and a Rust `char` cannot hold; or what `chr` raises
/// ([`ChrError`]).
pub(crate) fn chr(digits: &str, radix: u32) -> Result<u32, ChrError> {
    debug_assert!(
        !digits.is_empty() && digits.chars().all(|c| c.is_digit(radix)),
        "chr() takes the digits of an int"
    );
    // Read saturating: a value past `u64::MAX` is past `INT_MAX` as well.
    let value = digits.chars().fold(0u64, |value, c| {
        value
            .saturating_mul(u64::from(radix))
            .saturating_add(u64::from(c.to_digit(radix).unwrap_or(0)))
    });
    match u32::try_from(value) {
        Ok(point) if point <= 0x10_FFFF => Ok(point),
        Ok(point) if point <= 0x7FFF_FFFF => Err(ChrError::NotInRange),
        _ => Err(ChrError::Overflow),
    }
}

/// Python's `repr()` of a `float`, which `str()` and an f-string's
/// `{value}` give too (CPython's `float_repr`: `PyOS_double_to_string(x,
/// 'r', 0, Py_DTSF_ADD_DOT_0)`): the shortest digits that read back as the
/// value, as Rust's `{:e}` writes them; in exponent form when the decimal
/// exponent is below -4 or at least 16, the digits after the first behind
/// a `.` and the exponent signed, of two digits at least (`1e+16`,
/// `1.5e-05`); in fixed form otherwise, with `.0` after an integral value
/// (`1000.0`). The sign of a negative zero is kept, and the special values
/// are `inf`, `-inf` and `nan`, whatever the sign of a NaN.
pub(crate) fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(magnitude) => ("-", magnitude),
        None => ("", mantissa),
    };
    let digits: String = mantissa.chars().filter(|&c| c != '.').collect();
    if !(-4..16).contains(&exponent) {
        let mantissa = match digits.split_at(1) {
            (first, "") => first.to_string(),
            (first, rest) => format!("{first}.{rest}"),
        };
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        return format!(
            "{sign}{mantissa}e{exponent_sign}{:02}",
            exponent.unsigned_abs()
        );
    }
    if exponent < 0 {
        let zeros = "0".repeat(exponent.unsigned_abs() as usize - 1);
        return format!("{sign}0.{zeros}{digits}");
    }
    let whole = exponent.unsigned_abs() as usize + 1;
    if digits.len() > whole {
        format!("{sign}{}.{}", &digits[..whole], &digits[whole..])
    } else {
        format!("{sign}{digits}{}.0", "0".repeat(whole - digits.len()))
    }
}

/// Python's `str()` of a number a `conf.py` literal holds, which `repr()`
/// gives too, read as a JSON number: an `int`'s digits, and a `float`'s
/// `repr()` ([`float_repr`]), which serde_json's own text is not (`0.00001`
/// and `1e-7` for `1e-05` and `1e-07`).
pub(crate) fn number_str(number: &serde_json::Number) -> String {
    match number.as_f64() {
        Some(value) if number.is_f64() => float_repr(value),
        _ => number.to_string(),
    }
}

/// The text `float()` and `int()` read in place of the `str` `text`
/// (CPython's `_PyUnicode_TransformDecimalAndSpaceToASCII`): each
/// character below DEL as it is, whitespace past ASCII a space, a decimal
/// digit of any script ([`decimal_digit_value`]) its ASCII digit, and the
/// first other character a `?`, which ends the text. Neither reads a `?`,
/// so such a text is rejected.
fn number_ascii(text: &str) -> String {
    let mut ascii = String::with_capacity(text.len());
    for c in text.chars() {
        if u32::from(c) < 127 {
            ascii.push(c);
        } else if crate::utils::py_isspace(c) {
            ascii.push(' ');
        } else if let Some(digit) =
            decimal_digit_value(c).and_then(|value| char::from_digit(value, 10))
        {
            ascii.push(digit);
        } else {
            ascii.push('?');
            break;
        }
    }
    ascii
}

/// CPython's `Py_ISSPACE`, the whitespace `float()` and `int()` strip
/// around the text [`number_ascii`] gives: ASCII's six, not the C0
/// separators `\x1c`-`\x1f` that `str.isspace` also admits.
fn ascii_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{json, Value as Json};

    use super::*;

    /// The last line of the traceback Python prints for `error` (`KeyError:
    /// 'nope'`, the crash's own line), or the field text of a value the port
    /// does not model.
    fn error_line(error: FormatError) -> String {
        match error {
            FormatError::KeyError(key) => format!("KeyError: {}", py_repr_str(&key)),
            FormatError::TypeError(crash) | FormatError::Crash(crash) => crash.to_string(),
            FormatError::NotModelled(text) => format!("not modelled: {text}"),
        }
    }

    /// `title.format(name='Cap', number='1')`.
    fn cap_format(title: &str) -> Result<String, FormatError> {
        format(
            title,
            &[("name", Value::Str("Cap")), ("number", Value::Str("1"))],
        )
    }

    /// [`cap_format`] as the hand-written rows below read it: the text in
    /// quotes, or the exception's line.
    fn cap(title: &str) -> String {
        match cap_format(title) {
            Ok(text) => format!("{text:?}"),
            Err(error) => error_line(error),
        }
    }

    /// `str.format` reads a format from the left, each field filled before
    /// the next is read: literal braces, a `KeyError` for a name it is not
    /// given (the name's first part, before an attribute, a conversion or
    /// a spec), which Sphinx catches, and the `IndexError` of a positional
    /// field and the `ValueError`s of a format it cannot read, which
    /// Sphinx does not (decision 13).
    //
    // oracle (CPython 3.12, the locked core profile): each title below
    //   `.format(number='1', name='Cap')`, the result or the exception.
    #[test]
    fn new_style_formats_fail_as_python_does() {
        let single_close = "ValueError: Single '}' encountered in format string";
        let unclosed = "ValueError: expected '}' before end of string";
        let index = |n: u32| {
            format!("IndexError: Replacement index {n} out of range for positional args tuple")
        };
        // Twenty ARABIC-INDIC DIGIT NINEs, then a character that is no digit.
        let many_digits = format!("{{{}x}}", "\u{669}".repeat(20));
        for (title, expected) in [
            ("{name} {number}", "\"Cap 1\"".to_string()),
            ("}}{{", "\"}{\"".to_string()),
            ("a{{", "\"a{\"".to_string()),
            ("{{number}}", "\"{number}\"".to_string()),
            ("{{x}", single_close.to_string()),
            ("a}", single_close.to_string()),
            ("{number}}", single_close.to_string()),
            (
                "a{",
                "ValueError: Single '{' encountered in format string".to_string(),
            ),
            ("{number", unclosed.to_string()),
            ("{0", unclosed.to_string()),
            ("{x[}", unclosed.to_string()),
            (
                "{x!r",
                "ValueError: unmatched '{' in format spec".to_string(),
            ),
            (
                "{number:{",
                "ValueError: unmatched '{' in format spec".to_string(),
            ),
            (
                "{x!",
                "ValueError: end of string while looking for conversion specifier".to_string(),
            ),
            (
                "{x!rr}",
                "ValueError: expected ':' after conversion specifier".to_string(),
            ),
            (
                "{ {",
                "ValueError: unexpected '{' in field name".to_string(),
            ),
            ("{0}", index(0)),
            ("{}", index(0)),
            ("{number} {}", index(0)),
            ("{00}", index(0)),
            ("{0!r}", index(0)),
            ("{[0]}", index(0)),
            ("{!r}", index(0)),
            ("{1}", index(1)),
            (
                "{99999999999999999999}",
                "ValueError: Too many decimal digits in format string".to_string(),
            ),
            ("{-1}", "KeyError: '-1'".to_string()),
            ("{x.}", "KeyError: 'x'".to_string()),
            ("{x:{0}}", "KeyError: 'x'".to_string()),
            ("{x]}", "KeyError: 'x]'".to_string()),
            ("{number }", "KeyError: 'number '".to_string()),
            ("{nope} {", "KeyError: 'nope'".to_string()),
            ("{12x}", "KeyError: '12x'".to_string()),
            (
                "{922337203685477580x}",
                "KeyError: '922337203685477580x'".to_string(),
            ),
            // Digits are read from the left: past `isize::MAX` before the
            // character that is no digit.
            (
                "{99999999999999999999x}",
                "ValueError: Too many decimal digits in format string".to_string(),
            ),
            // A decimal digit of any script is a digit
            // (`Py_UNICODE_TODECIMAL`): ARABIC-INDIC DIGIT THREE and ZERO,
            // MATHEMATICAL BOLD DIGIT ZERO; SUPERSCRIPT TWO is no decimal
            // digit.
            ("{\u{663}}", index(3)),
            ("{\u{660}}", index(0)),
            ("{1\u{660}}", index(10)),
            ("{\u{1d7ce}}", index(0)),
            (
                many_digits.as_str(),
                "ValueError: Too many decimal digits in format string".to_string(),
            ),
            (
                "{\u{661}\u{662}x}",
                "KeyError: '\u{661}\u{662}x'".to_string(),
            ),
            ("{\u{b2}}", "KeyError: '\u{b2}'".to_string()),
        ] {
            assert_eq!(cap(title), expected, "{title:?}");
        }
    }

    /// The rest of a field, after the keyword its first part names, is
    /// looked up on the string the keyword gives, one `.attr` or `[item]`
    /// at a time, each before the next is read (CPython's
    /// `FieldNameIterator`): an empty one, or anything but `.` or `[` after
    /// a `]`, is a `ValueError`; an item of digits the string's character
    /// at that index or an `IndexError`, any other item a `TypeError`; an
    /// attribute `str` lacks an `AttributeError`, which Python's traceback
    /// completes with the closest name `str` has, if one is close enough.
    /// All of it before the field's conversion and format spec are applied.
    //
    // oracle (CPython 3.12, the locked core profile): each title below
    //   `.format(number='1', name='Cap')`, the result or the last line of
    //   `traceback.TracebackException.format_exception_only()`, which
    //   Sphinx's crash report prints.
    #[test]
    fn format_fields_look_up_the_rest_on_the_string_as_python_does() {
        let empty = "ValueError: Empty attribute in format string";
        let after_index = "ValueError: Only '.' or '[' may follow ']' in format field specifier";
        let out_of_range = "IndexError: string index out of range";
        let too_many_digits = "ValueError: Too many decimal digits in format string";
        let string_index = "TypeError: string indices must be integers, not 'str'";
        let no_attribute =
            |name: &str| format!("AttributeError: 'str' object has no attribute '{name}'");
        let did_you_mean = |name: &str, suggestion: &str| {
            format!("{}. Did you mean: '{suggestion}'?", no_attribute(name))
        };
        // Twenty ARABIC-INDIC DIGIT NINEs.
        let many_digits = format!("{{name[{}]}}", "\u{669}".repeat(20));
        for (title, expected) in [
            // An item of decimal digits of any script is an index
            // (`Py_UNICODE_TODECIMAL`): ARABIC-INDIC DIGIT ZERO, ONE and
            // NINE, MATHEMATICAL BOLD DIGIT ZERO; SUPERSCRIPT TWO is no
            // decimal digit.
            ("{number[\u{660}]}", "\"1\"".to_string()),
            ("{number[0\u{660}]}", "\"1\"".to_string()),
            ("{name[\u{661}]}{number[\u{660}]}", "\"a1\"".to_string()),
            ("{number[\u{1d7ce}]}", "\"1\"".to_string()),
            ("{name[\u{669}\u{669}]}", out_of_range.to_string()),
            (many_digits.as_str(), too_many_digits.to_string()),
            ("{number[\u{b2}]}", string_index.to_string()),
            ("{number[\u{660}.]}", string_index.to_string()),
            ("{number[\u{660}]x}", after_index.to_string()),
            ("{number.}", empty.to_string()),
            ("{number[]}", empty.to_string()),
            ("{number..}", empty.to_string()),
            ("{number[0].}", empty.to_string()),
            ("{number.:>3}", empty.to_string()),
            // The value of an attribute `str` has is not modelled, but the
            // next part is read before anything is looked up on it.
            ("{number.upper.}", empty.to_string()),
            ("{number[0]x}", after_index.to_string()),
            ("{number[0]]}", after_index.to_string()),
            ("{number[0]x.}", after_index.to_string()),
            ("{number[0]}", "\"1\"".to_string()),
            ("{number[00]}", "\"1\"".to_string()),
            ("{name[1]}", "\"a\"".to_string()),
            ("{name[2]}", "\"p\"".to_string()),
            ("{name[0][0]}", "\"C\"".to_string()),
            ("{number[0][0][0]}", "\"1\"".to_string()),
            ("{name[1]}{number[0]}", "\"a1\"".to_string()),
            ("{number[1]}", out_of_range.to_string()),
            ("{number[01]}", out_of_range.to_string()),
            ("{name[3]}", out_of_range.to_string()),
            ("{name[0][1]}", out_of_range.to_string()),
            ("{name} {number[9]}", out_of_range.to_string()),
            ("{number[5]!r}", out_of_range.to_string()),
            ("{number[5]:>3}", out_of_range.to_string()),
            ("{number[9223372036854775807]}", out_of_range.to_string()),
            ("{number[9223372036854775808]}", too_many_digits.to_string()),
            (
                "{number[99999999999999999999]}",
                too_many_digits.to_string(),
            ),
            (
                "{number[99999999999999999999x]}",
                too_many_digits.to_string(),
            ),
            ("{number[922337203685477580x]}", string_index.to_string()),
            ("{number[x]}", string_index.to_string()),
            ("{number[-1]}", string_index.to_string()),
            ("{number[ 0]}", string_index.to_string()),
            ("{number[0}]}", string_index.to_string()),
            ("{number[[]}", string_index.to_string()),
            ("{number[x].}", string_index.to_string()),
            ("{number[x]!r}", string_index.to_string()),
            ("{number.real}", no_attribute("real")),
            ("{number.real.}", no_attribute("real")),
            ("{number[0].real}", no_attribute("real")),
            ("{number.x.}", no_attribute("x")),
            ("{number.a'b}", no_attribute("a'b")),
            ("{number.a]b}", no_attribute("a]b")),
            ("{number.1}", no_attribute("1")),
            ("{number.st}", no_attribute("st")),
            ("{number.jion}", no_attribute("jion")),
            ("{number.__eq}", no_attribute("__eq")),
            ("{number.UPPER}", no_attribute("UPPER")),
            ("{number.SPLIT}", no_attribute("SPLIT")),
            ("{number.cASEFOLD}", no_attribute("cASEFOLD")),
            ("{number.\u{130}}", no_attribute("\u{130}")),
            ("{number.\u{131}}", no_attribute("\u{131}")),
            (
                "{number.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}",
                no_attribute("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            ),
            ("{number.uper}", did_you_mean("uper", "upper")),
            ("{number.uppe}", did_you_mean("uppe", "upper")),
            ("{number.Upper}", did_you_mean("Upper", "upper")),
            ("{number.Split}", did_you_mean("Split", "split")),
            ("{number.spliT}", did_you_mean("spliT", "split")),
            ("{number.\u{17f}plit}", did_you_mean("\u{17f}plit", "split")),
            ("{number.isascIi}", did_you_mean("isascIi", "isascii")),
            ("{name.lowr}", did_you_mean("lowr", "lower")),
            ("{number.strp}", did_you_mean("strp", "strip")),
            ("{number.lstrp}", did_you_mean("lstrp", "lstrip")),
            ("{number.rsplt}", did_you_mean("rsplt", "rsplit")),
            ("{number.zfil}", did_you_mean("zfil", "zfill")),
            ("{number.titl}", did_you_mean("titl", "title")),
            ("{number.findd}", did_you_mean("findd", "find")),
            ("{number.joinn}", did_you_mean("joinn", "join")),
            ("{number.counts}", did_you_mean("counts", "count")),
            ("{number.centre}", did_you_mean("centre", "center")),
            ("{number.rjustt}", did_you_mean("rjustt", "rjust")),
            ("{number.endswit}", did_you_mean("endswit", "endswith")),
            ("{number.partitio}", did_you_mean("partitio", "partition")),
            (
                "{number.expandtab}",
                did_you_mean("expandtab", "expandtabs"),
            ),
            (
                "{number.splitline}",
                did_you_mean("splitline", "splitlines"),
            ),
            (
                "{number.formatmap}",
                did_you_mean("formatmap", "format_map"),
            ),
            (
                "{number.removeprefixx}",
                did_you_mean("removeprefixx", "removeprefix"),
            ),
            (
                "{number.rremoveprefix}",
                did_you_mean("rremoveprefix", "removeprefix"),
            ),
            ("{number.__dict__}", did_you_mean("__dict__", "__dir__")),
            ("{number.__lenn__}", did_you_mean("__lenn__", "__len__")),
            ("{number.__neg__}", did_you_mean("__neg__", "__ne__")),
            ("{number.__radd__}", did_you_mean("__radd__", "__add__")),
            ("{number.__iadd__}", did_you_mean("__iadd__", "__add__")),
            (
                "{number.__getattribute}",
                did_you_mean("__getattribute", "__getattribute__"),
            ),
            (
                "{number.__init_subclas__}",
                did_you_mean("__init_subclas__", "__init_subclass__"),
            ),
            (
                "{number.__subclasshookk__}",
                did_you_mean("__subclasshookk__", "__subclasshook__"),
            ),
        ] {
            assert_eq!(cap(title), expected, "{title:?}");
        }
    }

    /// `_resolve_numref_xref`'s two styles, as it calls them: `title %
    /// fignum`, a `TypeError` for a title with no conversion or with two;
    /// and `title.format(name=figname, number=fignum)`, or
    /// `title.format(number=fignum)` for an empty caption, where `{{` and
    /// `}}` are a literal brace and a field that names neither is a
    /// `KeyError`. Moved from `src/env/resolve.rs` with the port; the
    /// resolver's own tests drive `Resolver::resolve_xref`.
    #[test]
    fn numref_titles_format_in_both_styles() {
        let numref_format = |title: &str, figname: Option<&str>, fignum: &str| match figname {
            Some(figname) => format(
                title,
                &[
                    ("name", Value::Str(figname)),
                    ("number", Value::Str(fignum)),
                ],
            ),
            None => format(title, &[("number", Value::Str(fignum))]),
        };
        assert_eq!(percent("Fig. %s", "1.2").unwrap(), "Fig. 1.2");
        assert!(
            matches!(percent("Fig.", "1"), Err(FormatError::TypeError(_))),
            "no conversion: TypeError"
        );
        assert!(
            matches!(percent("%s %s", "1"), Err(FormatError::TypeError(_))),
            "two conversions for one argument: TypeError"
        );
        assert_eq!(
            numref_format("Custom {name} number {number}", Some("Cap"), "1").unwrap(),
            "Custom Cap number 1"
        );
        assert_eq!(
            numref_format("Table {number}", None, "3").unwrap(),
            "Table 3"
        );
        assert!(matches!(
            numref_format("{nope}", Some("Cap"), "1"),
            Err(FormatError::KeyError(key)) if key == "nope"
        ));
        for (title, figname, expected) in [
            ("Tab. {{x}} {number}", None, "Tab. {x} 1"),
            ("Pic {{n}} {number}}}", None, "Pic {n} 1}"),
            (
                "Custom {{name}} {name} {number}",
                Some("Cap"),
                "Custom {name} Cap 1",
            ),
            ("}}{{", None, "}{"),
        ] {
            assert_eq!(
                numref_format(title, figname, "1").unwrap(),
                expected,
                "{title}"
            );
        }
        assert!(matches!(
            numref_format("{nope}}}", None, "1"),
            Err(FormatError::KeyError(key)) if key == "nope"
        ));
    }

    /// Not modelled (T19b's divergence list): the value of an attribute
    /// that the value has (`{number.upper}`, `{number.__class__}`), which
    /// Python goes on with, a bound method mostly, whose text names its
    /// address in memory. Rendered without a spec, or after `!s`, `!r` or
    /// `!a` with a spec a `str` takes, the port reads on past it as Python
    /// does, so that what the rest of the format raises, which does not
    /// depend on that text, is raised: a later field's lookup, conversion,
    /// spec and rendering, the parser's errors, a nested field of the spec.
    /// After a conversion the value is a `str` whose text the port does not
    /// know, and its spec raises what `str.__format__` raises whatever the
    /// text: the spec's own errors, a type, sign, `z`, `#` or `=` a `str`
    /// does not take, a width no allocation holds. The port stops at once
    /// where the next step needs the value itself: a lookup on it, a spec
    /// built from its text, or rendering it with a spec and no conversion,
    /// whose outcome depends on the value's type. Each of those, and a
    /// format that otherwise completes, is [`FormatError::NotModelled`]
    /// with the whole text of the first field of the format that reaches
    /// such a value or builds its spec from one. What CPython 3.12 gives
    /// for each row not modelled is in the comment above it.
    //
    // oracle (CPython 3.12, the locked core profile): each title below
    //   `.format(number='1', name='Cap')`, and each URL
    //   `.format_map({'page': 'ls', 'path': 'ls(1)', 'section': '1'})`,
    //   or over a `section` of `None`.
    #[test]
    fn the_value_of_an_attribute_a_value_has_is_not_modelled() {
        let not_modelled = |text: &str| format!("not modelled: {text}");
        let not_allowed =
            |what: &str| format!("ValueError: {what} not allowed in string format specifier");
        let unknown_code = |code: &str| {
            format!("ValueError: Unknown format code '{code}' for object of type 'str'")
        };
        let too_many_digits = "ValueError: Too many decimal digits in format string";
        for (title, expected) in [
            // The method's text.
            ("{number.upper}", not_modelled("number.upper")),
            // `TypeError: 'builtin_function_or_method' object is not
            // subscriptable`.
            ("{number.upper[0]}", not_modelled("number.upper[0]")),
            // `AttributeError: 'builtin_function_or_method' object has no
            // attribute 'x'`.
            ("{number.upper.x}", not_modelled("number.upper.x")),
            // The method's text, by `repr()`.
            ("{number.upper!r}", not_modelled("number.upper!r")),
            // `TypeError: unsupported format string passed to
            // builtin_function_or_method.__format__`.
            ("{number.upper:>3}", not_modelled("number.upper:>3")),
            // `<class 'str'>`.
            ("{number.__class__}", not_modelled("number.__class__")),
            // `1 ` and the method's text.
            ("{number} {number.upper!r}", not_modelled("number.upper!r")),
            // That `TypeError` again, after the method's text.
            (
                "{number.upper} {number.lower:>3}",
                not_modelled("number.upper"),
            ),
            // `ValueError: Invalid format specifier '<the method's text>'
            // for object of type 'str'`.
            (
                "{number:{number.upper}}",
                not_modelled("number:{number.upper}"),
            ),
            // That `TypeError` again.
            (
                "{number.upper:{number.upper}}",
                not_modelled("number.upper:{number.upper}"),
            ),
            // `s`, the first character of `str.__doc__`.
            ("{number.__doc__[0]}", not_modelled("number.__doc__[0]")),
            // `ValueError: Unknown format code 'd' for object of type
            // 'str'`: `str.__doc__` is a `str`, a type the port does not
            // know the value has.
            ("{number.__doc__:d}", not_modelled("number.__doc__:d")),
            // The method's text by `repr()` or `ascii()`, padded, or kept
            // whole.
            ("{number.upper!r:>3}", not_modelled("number.upper!r:>3")),
            ("{number.upper!a:*^60}", not_modelled("number.upper!a:*^60")),
            ("{number.upper!r:s}", not_modelled("number.upper!r:s")),
            (
                "{number.upper!r:\u{665}}",
                not_modelled("number.upper!r:\u{665}"),
            ),
            (
                "{number.upper!r:{number}}",
                not_modelled("number.upper!r:{number}"),
            ),
            // The padded text, then the method's text.
            (
                "{number.upper!r:>3} {number.lower}",
                not_modelled("number.upper!r:>3"),
            ),
            // `TypeError: unsupported format string passed to
            // builtin_function_or_method.__format__`, after the padded text.
            (
                "{number.upper!r:>3} {number.lower:>3}",
                not_modelled("number.upper!r:>3"),
            ),
            // `ValueError: Invalid format specifier '<the method's text by
            // repr()>' for object of type 'str'`.
            (
                "{number:{number.upper!r}}",
                not_modelled("number:{number.upper!r}"),
            ),
            (
                "{number.upper!r:{number.upper}}",
                not_modelled("number.upper!r:{number.upper}"),
            ),
            // After `!s`, `!r` or `!a`, what `str.__format__` raises for
            // the spec, whatever the text.
            ("{number.upper!r:d}", unknown_code("d")),
            ("{number.upper!a:x}", unknown_code("x")),
            ("{number.upper!r:\u{e9}}", unknown_code("\\xe9")),
            ("{number.__class__!r:d}", unknown_code("d")),
            ("{number.__doc__!r:d}", unknown_code("d")),
            ("{number.upper!s:+}", not_allowed("Sign")),
            ("{number.upper!a: }", not_allowed("Space")),
            (
                "{number.upper!r:z}",
                not_allowed("Negative zero coercion (z)"),
            ),
            ("{number.upper!s:#}", not_allowed("Alternate form (#)")),
            ("{number.upper!a:=5}", not_allowed("'=' alignment")),
            (
                "{number.upper!r:,}",
                "ValueError: Cannot specify ',' with 's'.".to_string(),
            ),
            (
                "{number.upper!s:_}",
                "ValueError: Cannot specify '_' with 's'.".to_string(),
            ),
            (
                "{number.upper!a:,_}",
                "ValueError: Cannot specify both ',' and '_'.".to_string(),
            ),
            (
                "{number.upper!r:.}",
                "ValueError: Format specifier missing precision".to_string(),
            ),
            (
                "{number.upper!s:ss}",
                "ValueError: Invalid format specifier 'ss' for object of type 'str'".to_string(),
            ),
            (
                "{number.upper!r:9223372036854775808}",
                too_many_digits.to_string(),
            ),
            (
                "{number.upper!r:.9223372036854775808}",
                too_many_digits.to_string(),
            ),
            (
                "{number.upper!r:1000000000000000000}",
                "MemoryError".to_string(),
            ),
            (
                "{number.upper!s:>9223372036854775807}",
                "MemoryError".to_string(),
            ),
            (
                "{number.upper!a:\u{1f600}^1000000000000000000}",
                "MemoryError".to_string(),
            ),
            // After a spec a `str` takes, what the rest of the format
            // raises.
            ("{number.upper!r:>3} {nope}", "KeyError: 'nope'".to_string()),
            (
                "{number} {number.upper!r:>3} {nope}",
                "KeyError: 'nope'".to_string(),
            ),
            (
                "{number.__class__!s:>3} {nope}",
                "KeyError: 'nope'".to_string(),
            ),
            ("{number.upper!r:>3} {number:d}", unknown_code("d")),
            ("{number.upper!r:>3} {number.lower!s:d}", unknown_code("d")),
            (
                "{number.upper!s:<5} {0}",
                "IndexError: Replacement index 0 out of range for positional args tuple"
                    .to_string(),
            ),
            (
                "{number.upper!a:^7} {name.real}",
                "AttributeError: 'str' object has no attribute 'real'".to_string(),
            ),
            // What Python raises before it needs the value.
            (
                "{number.upper.}",
                "ValueError: Empty attribute in format string".to_string(),
            ),
            (
                "{number.upper!z}",
                "ValueError: Unknown conversion specifier z".to_string(),
            ),
            (
                "{number.upper:{0}}",
                "IndexError: Replacement index 0 out of range for positional args tuple"
                    .to_string(),
            ),
            ("{number.upper:{nope}}", "KeyError: 'nope'".to_string()),
            (
                "{number.upper:{number:{number}}}",
                "ValueError: Max string recursion exceeded".to_string(),
            ),
            (
                "{number:{number.upper:{0}}}",
                "ValueError: Max string recursion exceeded".to_string(),
            ),
            // What the rest of the format raises, read past the value.
            ("{number.upper} {nope}", "KeyError: 'nope'".to_string()),
            (
                "{number.upper} {0}",
                "IndexError: Replacement index 0 out of range for positional args tuple"
                    .to_string(),
            ),
            (
                "{number.upper} {}",
                "IndexError: Replacement index 0 out of range for positional args tuple"
                    .to_string(),
            ),
            (
                "Eq {number.upper} {number:d}",
                "ValueError: Unknown format code 'd' for object of type 'str'".to_string(),
            ),
            (
                "{number.upper} {number!z}",
                "ValueError: Unknown conversion specifier z".to_string(),
            ),
            (
                "{number.upper} {number:{0}}",
                "IndexError: Replacement index 0 out of range for positional args tuple"
                    .to_string(),
            ),
            (
                "{number.upper} {name.real}",
                "AttributeError: 'str' object has no attribute 'real'".to_string(),
            ),
            (
                "{number.upper} {number[x]}",
                "TypeError: string indices must be integers, not 'str'".to_string(),
            ),
            (
                "{number.upper} {number[5]}",
                "IndexError: string index out of range".to_string(),
            ),
            (
                "{number.upper} {number:9223372036854775808}",
                "ValueError: Too many decimal digits in format string".to_string(),
            ),
            (
                "{number.upper} {number:1000000000000000000}",
                "MemoryError".to_string(),
            ),
            (
                "{number.upper}}",
                "ValueError: Single '}' encountered in format string".to_string(),
            ),
            (
                "{number.upper} {",
                "ValueError: Single '{' encountered in format string".to_string(),
            ),
            (
                "{number.upper} {number",
                "ValueError: expected '}' before end of string".to_string(),
            ),
            ("{nope} {number.upper}", "KeyError: 'nope'".to_string()),
            (
                "{number:{number.upper}{nope}}",
                "KeyError: 'nope'".to_string(),
            ),
        ] {
            assert_eq!(cap(title), expected, "{title}");
        }
        let man = |url: &str, section: Value<'_>| {
            let info = [
                ("page", Value::Str("ls")),
                ("path", Value::Str("ls(1)")),
                ("section", section),
            ];
            match format_map(url, &info) {
                Ok(text) => format!("{text:?}"),
                Err(error) => error_line(error),
            }
        };
        let section = Value::Str("1");
        for (url, section, expected) in [
            // `<class 'NoneType'>`.
            (
                "{section.__class__}",
                Value::None,
                not_modelled("section.__class__"),
            ),
            // The URL with the method's text and `  1`.
            (
                "https://m/{page.upper}/{section:>3}",
                section,
                not_modelled("page.upper"),
            ),
            // The URL with the method's text by `str()`, padded.
            (
                "https://m/{page.upper!s:>3}",
                section,
                not_modelled("page.upper!s:>3"),
            ),
            // `https://m/<class 'NoneType'>/None`.
            (
                "https://m/{section.__class__!r:>3}/{section}",
                Value::None,
                not_modelled("section.__class__!r:>3"),
            ),
            ("https://m/{page.upper!r:d}", section, unknown_code("d")),
            (
                "https://m/{page.upper!a:,}",
                section,
                "ValueError: Cannot specify ',' with 's'.".to_string(),
            ),
            (
                "https://m/{section.__class__!s:d}",
                Value::None,
                unknown_code("d"),
            ),
            (
                "https://m/{page.upper!r:>3}/{nope}",
                section,
                "KeyError: 'nope'".to_string(),
            ),
            (
                "https://m/{page.upper!r:>3}/{0}",
                section,
                "ValueError: Format string contains positional fields".to_string(),
            ),
            (
                "https://m/{page.upper!r:<4}/{section:>3}",
                Value::None,
                "TypeError: unsupported format string passed to NoneType.__format__".to_string(),
            ),
            (
                "https://m/{page.upper}/{nope}",
                section,
                "KeyError: 'nope'".to_string(),
            ),
            (
                "https://m/{page.upper}/{0}",
                section,
                "ValueError: Format string contains positional fields".to_string(),
            ),
            (
                "https://m/{page.upper}/{}",
                section,
                "ValueError: Format string contains positional fields".to_string(),
            ),
            (
                "https://m/{page.upper}/{section!z}",
                section,
                "ValueError: Unknown conversion specifier z".to_string(),
            ),
            (
                "https://m/{page.upper}/{section:>3}",
                Value::None,
                "TypeError: unsupported format string passed to NoneType.__format__".to_string(),
            ),
        ] {
            assert_eq!(man(url, section), expected, "{url}");
        }
    }

    /// `tests/fixtures/pyformat_units.json`, which
    /// `tools/gen_pyformat_fixture.py` records from CPython 3.12.2.
    fn fixture() -> Json {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/pyformat_units.json");
        let text = std::fs::read_to_string(path).expect("fixture readable");
        serde_json::from_str(&text).expect("fixture parses")
    }

    fn str_of(value: &Json) -> &str {
        value
            .as_str()
            .unwrap_or_else(|| panic!("a string: {value}"))
    }

    fn rows(value: &Json) -> &Vec<Json> {
        value
            .as_array()
            .unwrap_or_else(|| panic!("a list of rows: {value}"))
    }

    /// The argument sets of a fixture section (`format_args`,
    /// `format_map_args`) by name: each argument's name and value, `None`
    /// for a JSON `null`.
    fn argument_sets(value: &Json) -> BTreeMap<String, Vec<(String, Option<String>)>> {
        value
            .as_object()
            .unwrap_or_else(|| panic!("argument sets: {value}"))
            .iter()
            .map(|(set, arguments)| {
                let arguments = arguments
                    .as_object()
                    .unwrap_or_else(|| panic!("an argument set: {arguments}"))
                    .iter()
                    .map(|(name, value)| (name.clone(), value.as_str().map(str::to_string)))
                    .collect();
                (set.clone(), arguments)
            })
            .collect()
    }

    /// An argument set as the port takes it.
    fn values(arguments: &[(String, Option<String>)]) -> Vec<(&str, Value<'_>)> {
        arguments
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str(),
                    value.as_deref().map_or(Value::None, Value::Str),
                )
            })
            .collect()
    }

    /// The port's result as a fixture row records Python's: `{"ok": text}`
    /// or `{"raises": line}`. An exception the port routes to the wrong
    /// kind (a `TypeError` that is not `FormatError::TypeError`, a `KeyError`
    /// or `TypeError` in `FormatError::Crash`) reads as `misrouted`, which
    /// no row holds.
    fn outcome(result: Result<String, FormatError>) -> Json {
        match result {
            Ok(text) => json!({ "ok": text }),
            Err(FormatError::TypeError(crash)) if crash.exception != "TypeError" => {
                json!({ "misrouted": crash.to_string() })
            }
            Err(FormatError::Crash(crash))
                if matches!(crash.exception, "KeyError" | "TypeError") =>
            {
                json!({ "misrouted": crash.to_string() })
            }
            Err(error @ FormatError::NotModelled(_)) => json!({ "not": error_line(error) }),
            Err(error) => json!({ "raises": error_line(error) }),
        }
    }

    /// A fixture row's outcome: its `ok` or `raises`.
    fn expected(row: &Json) -> Json {
        let mut expected = serde_json::Map::new();
        for key in ["ok", "raises"] {
            if let Some(value) = row.get(key) {
                expected.insert(key.to_string(), value.clone());
            }
        }
        Json::Object(expected)
    }

    /// A row's `text`, `[[piece, count], ...]`, joined.
    fn joined(value: &Json) -> String {
        rows(value)
            .iter()
            .map(|pair| {
                let count = pair[1]
                    .as_u64()
                    .unwrap_or_else(|| panic!("a count: {pair}"));
                str_of(&pair[0]).repeat(usize::try_from(count).expect("a count that fits"))
            })
            .collect()
    }

    /// `int(text)` as a fixture row records it.
    fn int_outcome(text: &str) -> Json {
        match int(text) {
            Ok((negative, digits)) => {
                json!({ "ok": if negative { format!("-{digits}") } else { digits } })
            }
            Err(message) => json!({ "raises": format!("ValueError: {message}") }),
        }
    }

    /// Every row of `tests/fixtures/pyformat_units.json` through the port:
    /// `str.format` with keywords and `str.format_map` over a manual-page
    /// `info`, each hand-written format under each argument set and the
    /// random ones under theirs; `%` with one `str`; `int()` and `float()`;
    /// `chr()` of a code `int()` reads in base 10 or 16; `repr()` of a
    /// float. The text Python renders, or the exception it raises, its
    /// class and its message, which also decides the error's kind
    /// ([`FormatError`]).
    #[test]
    fn every_fixture_row_matches_cpython() {
        let fixture = fixture();
        let format_sets = argument_sets(&fixture["format_args"]);
        let format_map_sets = argument_sets(&fixture["format_map_args"]);
        let mut checked = 0usize;
        let mut mismatches = Vec::new();
        let mut check = |what: String, ours: Json, python: Json| {
            checked += 1;
            if ours != python {
                mismatches.push(format!("{what}\n  ours   {ours}\n  python {python}"));
            }
        };
        for row in rows(&fixture["format"]) {
            let (fmt, set) = (str_of(&row["format"]), str_of(&row["args"]));
            check(
                format!("{fmt:?}.format(**{set})"),
                outcome(format(fmt, &values(&format_sets[set]))),
                expected(row),
            );
        }
        for row in rows(&fixture["format_map"]) {
            let (fmt, set) = (str_of(&row["format"]), str_of(&row["args"]));
            check(
                format!("{fmt:?}.format_map({set})"),
                outcome(format_map(fmt, &values(&format_map_sets[set]))),
                expected(row),
            );
        }
        for row in rows(&fixture["percent"]) {
            let (fmt, arg) = (str_of(&row["format"]), str_of(&row["arg"]));
            check(
                format!("{fmt:?} % {arg:?}"),
                outcome(percent(fmt, arg)),
                expected(row),
            );
        }
        for (section, sets, count) in [
            ("random_format", &format_sets, 1000),
            ("random_format_map", &format_map_sets, 400),
        ] {
            let names: Vec<&str> = rows(&fixture[section]["args"]).iter().map(str_of).collect();
            let section_rows = rows(&fixture[section]["rows"]);
            assert_eq!(section_rows.len(), count, "{section}");
            for row in section_rows {
                let fmt = str_of(&row["format"]);
                for (set, python) in names.iter().zip(rows(&row["out"])) {
                    let arguments = values(&sets[*set]);
                    let ours = if section == "random_format" {
                        format(fmt, &arguments)
                    } else {
                        format_map(fmt, &arguments)
                    };
                    check(
                        format!("{section} {fmt:?} under {set}"),
                        outcome(ours),
                        python.clone(),
                    );
                }
            }
        }
        let args: Vec<&str> = rows(&fixture["random_percent"]["args"])
            .iter()
            .map(str_of)
            .collect();
        let percent_rows = rows(&fixture["random_percent"]["rows"]);
        assert_eq!(percent_rows.len(), 600, "random_percent");
        for row in percent_rows {
            let fmt = str_of(&row["format"]);
            for (arg, python) in args.iter().zip(rows(&row["out"])) {
                check(
                    format!("random {fmt:?} % {arg:?}"),
                    outcome(percent(fmt, arg)),
                    python.clone(),
                );
            }
        }
        for row in rows(&fixture["int"]) {
            let text = joined(&row["text"]);
            check(format!("int({text:?})"), int_outcome(&text), expected(row));
        }
        for row in rows(&fixture["float"]) {
            let text = joined(&row["text"]);
            let ours = match float(&text) {
                Ok(value) => json!({ "bits": format!("{:016x}", value.to_bits()) }),
                Err(message) => json!({ "raises": format!("ValueError: {message}") }),
            };
            let python = match row.get("bits") {
                Some(bits) => json!({ "bits": bits }),
                None => expected(row),
            };
            check(format!("float({text:?})"), ours, python);
        }
        for row in rows(&fixture["chr"]) {
            let text = joined(&row["text"]);
            let base = row["base"]
                .as_u64()
                .and_then(|base| u32::try_from(base).ok())
                .unwrap_or_else(|| panic!("a base: {row}"));
            // `code.isdigit()` lets no sign through to `int()`.
            let digits = if base == 10 {
                int(&text).map(|(_, digits)| digits)
            } else {
                Ok(text.clone())
            };
            let ours = match digits.map(|digits| chr(&digits, base)) {
                Err(message) => json!({ "raises": format!("ValueError: {message}") }),
                Ok(Ok(point)) => json!({ "ok": point }),
                Ok(Err(error)) => {
                    let exception = match error {
                        ChrError::Overflow => "OverflowError",
                        ChrError::NotInRange => "ValueError",
                    };
                    json!({ "raises": format!("{exception}: {}", error.message()) })
                }
            };
            check(format!("chr(int({text:?}, {base}))"), ours, expected(row));
        }
        for row in rows(&fixture["float_repr"]) {
            let bits = str_of(&row["bits"]);
            let value = u64::from_str_radix(bits, 16)
                .map(f64::from_bits)
                .unwrap_or_else(|_| panic!("IEEE 754 bits: {row}"));
            check(
                format!("repr(float.fromhex bits {bits})"),
                json!({ "ok": float_repr(value) }),
                expected(row),
            );
        }
        assert!(
            mismatches.is_empty(),
            "{} of {checked} rows differ from CPython 3.12.2:\n{}",
            mismatches.len(),
            mismatches
                .iter()
                .take(60)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// `int(c + "1" + c)` and `float(c + "1" + c)` for every code point
    /// `c` but the surrogates, against the runs where CPython 3.12.2 reads
    /// them and where it reads 1 (`tests/fixtures/pyformat_units.json`,
    /// `code_points`): the whitespace both strip around a number, Unicode's
    /// `White_Space` and not the C0 separators `\x1c`-`\x1f` that
    /// `str.isspace` also admits, and the decimal digits of any script both
    /// read.
    #[test]
    fn int_and_float_read_every_code_point_as_python_does() {
        let fixture = fixture();
        let runs = |key: &str| -> Vec<(u32, u32)> {
            rows(&fixture["code_points"][key])
                .iter()
                .map(|run| {
                    let bound = |at: usize| {
                        run[at]
                            .as_u64()
                            .and_then(|point| u32::try_from(point).ok())
                            .unwrap_or_else(|| panic!("a code point: {run}"))
                    };
                    (bound(0), bound(1))
                })
                .collect()
        };
        let within = |runs: &[(u32, u32)], point: u32| {
            runs.iter()
                .any(|&(start, end)| (start..=end).contains(&point))
        };
        let (int_ok, int_one) = (runs("int_ok"), runs("int_one"));
        let (float_ok, float_one) = (runs("float_ok"), runs("float_one"));
        let mut mismatches = Vec::new();
        for point in 0..=0x10_ffff_u32 {
            let Some(c) = char::from_u32(point) else {
                continue;
            };
            let text = format!("{c}1{c}");
            let int_read = int(&text);
            let ours = (
                int_read.is_ok(),
                int_read.is_ok_and(|(negative, digits)| !negative && digits == "1"),
            );
            if ours != (within(&int_ok, point), within(&int_one, point)) {
                mismatches.push(format!("int {point:#x}: ours {ours:?}"));
            }
            let float_read = float(&text);
            let ours = (float_read.is_ok(), float_read == Ok(1.0));
            if ours != (within(&float_ok, point), within(&float_one, point)) {
                mismatches.push(format!("float {point:#x}: ours {ours:?}"));
            }
        }
        assert!(
            mismatches.is_empty(),
            "{} code points differ from CPython 3.12.2:\n{}",
            mismatches.len(),
            mismatches
                .iter()
                .take(40)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
