//! Python's string formatting, as CPython 3.12.2 runs it, for the
//! configuration values Sphinx 9.1.0 formats with it: `str.format` over the
//! fields a `numfig_format`, an explicit `:numref:` title or
//! `math_eqref_format` names ([`format_new_style`]).

use crate::rst::digits::decimal_digit_value;
use crate::rst::SphinxCrash;

/// What Python's `str.format` raises out of [`format_new_style`].
#[derive(Debug)]
pub(crate) enum FormatError {
    /// `KeyError` for a field that names no argument, carrying the name,
    /// which both of Sphinx's callers catch and warn about.
    KeyError(String),
    /// The `TypeError` of a string indexed by a name (`{number[x]}`),
    /// which `_resolve_numref_xref` catches and `MathDomain.resolve_xref`
    /// does not: Sphinx 9.1.0 crashes there (decision 13). Located by the
    /// caller.
    TypeError(SphinxCrash),
    /// A `ValueError` for a format the method cannot read, an `IndexError`
    /// for a positional field (the method is given keywords only) or for
    /// an index past a string's end, or an `AttributeError` for an
    /// attribute `str` lacks, which neither caller catches: Sphinx 9.1.0
    /// crashes (decision 13). Located by the caller.
    Crash(SphinxCrash),
}

/// `title.format(name=..., number=...)` for the fields numfig formats can
/// name, and `math_eqref_format.format(number=...)`, read as Python's
/// `str.format` reads its format (`MarkupIterator` and `render_field` in
/// CPython's `Objects/stringlib/unicode_format.h`), from the left, each
/// field filled before the next is read:
///
/// - literal text, where `{{` and `}}` are a literal brace, and a single
///   `}`, or a `{` that ends the format, is a `ValueError`;
/// - a `{field}`, whose name ends at the first `}`, `:` or `!` outside a
///   `[...]` (a `{` in it is a `ValueError`); a `!` takes one conversion
///   character and then `}` or `:`, and a `:` a format spec up to the `}`
///   that balances its braces; a field that does not close is a
///   `ValueError`;
/// - the name's first part, up to a `.` or `[`, is a positional index
///   when it is empty or all decimal digits, of any script
///   ([`format_integer`]), an `IndexError` here, since no positional
///   argument is given (`Too many decimal digits` once the digits read
///   from the left pass `isize::MAX`, a `ValueError`, even before a
///   character that is no digit); otherwise a keyword, `number`, `name`
///   when `figname` is given, or a `KeyError`;
/// - the rest of the name is looked up on the string the keyword gives,
///   one `.attr` or `[item]` at a time, each before the next is read
///   (CPython's `FieldNameIterator` and the loop of `get_field_object`):
///   an empty `attr` or `item`, or anything but `.` or `[` after a `]`,
///   is a `ValueError`; an `item` of decimal digits, of any script, is the
///   string's character at that index, or an `IndexError` past its end
///   (or the `ValueError` past `isize::MAX`), and any other `item` a
///   `TypeError`; an `attr` that `str` lacks is an `AttributeError`, which
///   Python's traceback completes with the closest name `str` has
///   ([`str_attribute_error`]).
///
/// Not modelled (T19b's divergence list): a field's conversion and its
/// format spec, and the value of an `attr` that `str` has
/// (`{number.upper}`), which Python goes on with (a bound method, mostly,
/// whose text names its address in memory), each reading as a `KeyError`
/// of the whole field text once the lookups before it succeed — the
/// `.attr` or `[item]` after such an `attr` is still read, so a
/// `ValueError` there is raised, but nothing is looked up on its value.
pub(crate) fn format_new_style(
    title: &str,
    figname: Option<&str>,
    fignum: &str,
) -> Result<String, FormatError> {
    let chars: Vec<char> = title.chars().collect();
    let mut out = String::with_capacity(title.len());
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
            return Err(format_value_error(
                "Single '}' encountered in format string",
            ));
        }
        if at == chars.len() {
            return Err(format_value_error(
                "Single '{' encountered in format string",
            ));
        }
        let field = parse_format_field(&chars, &mut at)?;
        out.push_str(&fill_format_field(&field, figname, fignum)?);
    }
    Ok(out)
}

/// A `ValueError` `str.format` raises reading a format.
fn format_value_error(message: &str) -> FormatError {
    FormatError::Crash(SphinxCrash::python(
        "ValueError",
        message.to_string(),
        crate::rst::LogLocation::None,
    ))
}

/// One `{field}` of a format: its name, and its whole text between the
/// braces, which holds a conversion or a format spec when `qualified`.
struct FormatField {
    name: String,
    text: String,
    qualified: bool,
}

/// CPython's `parse_field`, from just after a field's `{` at `*at` to
/// just after its closing `}`.
fn parse_format_field(chars: &[char], at: &mut usize) -> Result<FormatField, FormatError> {
    let start = *at;
    let mut terminator = None;
    while let Some(&c) = chars.get(*at) {
        *at += 1;
        match c {
            '{' => return Err(format_value_error("unexpected '{' in field name")),
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
        return Err(format_value_error("expected '}' before end of string"));
    };
    let name: String = chars[start..*at - 1].iter().collect();
    let field = |at: usize| FormatField {
        name: name.clone(),
        text: chars[start..at - 1].iter().collect(),
        qualified: terminator != '}',
    };
    if terminator == '}' {
        return Ok(field(*at));
    }
    if terminator == '!' {
        if *at == chars.len() {
            return Err(format_value_error(
                "end of string while looking for conversion specifier",
            ));
        }
        *at += 1;
        if let Some(&c) = chars.get(*at) {
            *at += 1;
            if c == '}' {
                return Ok(field(*at));
            }
            if c != ':' {
                return Err(format_value_error(
                    "expected ':' after conversion specifier",
                ));
            }
        }
    }
    let mut depth = 1;
    while let Some(&c) = chars.get(*at) {
        *at += 1;
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(field(*at));
                }
            }
            _ => {}
        }
    }
    Err(format_value_error("unmatched '{' in format spec"))
}

/// CPython's `get_field_object` for `field`, with `number` and, when
/// `figname` is given, `name` as the keywords and no positional argument;
/// then the field's conversion and format spec, which are not modelled
/// ([`format_new_style`]).
fn fill_format_field(
    field: &FormatField,
    figname: Option<&str>,
    fignum: &str,
) -> Result<String, FormatError> {
    let name: Vec<char> = field.name.chars().collect();
    // `field_name_split`: the first part, up to a `.` or `[`.
    let first_len = name
        .iter()
        .position(|&c| c == '.' || c == '[')
        .unwrap_or(name.len());
    let index = format_integer(&name[..first_len])?;
    if first_len == 0 || index.is_some() {
        // An empty name is the next automatic index, which is always 0
        // here: the first positional field raises.
        return Err(FormatError::Crash(SphinxCrash::python(
            "IndexError",
            format!(
                "Replacement index {} out of range for positional args tuple",
                index.unwrap_or(0)
            ),
            crate::rst::LogLocation::None,
        )));
    }
    let first: String = name[..first_len].iter().collect();
    // The string the lookups have reached, or `None` once one has given
    // what is no string (an attribute `str` has), which is not modelled.
    let mut value = Some(match (first.as_str(), figname) {
        ("number", _) => fignum.to_string(),
        ("name", Some(figname)) => figname.to_string(),
        _ => return Err(FormatError::KeyError(first)),
    });
    let mut at = first_len;
    // `FieldNameIterator_next`, each part looked up before the next is
    // read.
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
                    // closes every `[` ([`parse_format_field`]).
                    return Err(format_value_error("Missing ']' in format string"));
                }
                at += 1;
                false
            }
            _ => {
                return Err(format_value_error(
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
            return Err(format_value_error("Empty attribute in format string"));
        }
        let Some(text) = value else {
            return Err(FormatError::KeyError(field.text.clone()));
        };
        value = if attribute {
            // `getattr(text, part)`.
            let part: String = part.iter().collect();
            if !STR_ATTRIBUTES.contains(&part.as_str()) {
                return Err(FormatError::Crash(str_attribute_error(&part)));
            }
            None
        } else {
            match index {
                // `PySequence_GetItem(text, index)`, the index never
                // negative here.
                Some(index) => match usize::try_from(index)
                    .ok()
                    .and_then(|index| text.chars().nth(index))
                {
                    Some(c) => Some(c.to_string()),
                    None => {
                        return Err(FormatError::Crash(SphinxCrash::python(
                            "IndexError",
                            "string index out of range".to_string(),
                            crate::rst::LogLocation::None,
                        )))
                    }
                },
                // `PyObject_GetItem(text, part)`.
                None => {
                    return Err(FormatError::TypeError(SphinxCrash::python(
                        "TypeError",
                        "string indices must be integers, not 'str'".to_string(),
                        crate::rst::LogLocation::None,
                    )))
                }
            }
        };
    }
    match value {
        Some(text) if !field.qualified => Ok(text),
        _ => Err(FormatError::KeyError(field.text.clone())),
    }
}

/// CPython's `get_integer` over a part of a field name: its value when
/// it is all decimal digits, of any script, as `Py_UNICODE_TODECIMAL`
/// reads them ([`decimal_digit_value`]: `{٣}` is the positional field
/// 3), read from the left, so that a value past `isize::MAX` is a
/// `ValueError` even before a character that is no digit; `None` for an
/// empty part or one with a character that is no digit.
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
            .ok_or_else(|| format_value_error("Too many decimal digits in format string"))?;
    }
    Ok(Some(value))
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

/// The `AttributeError` of `getattr` on a string for a `name` that `str`
/// lacks, as the last line of the traceback Sphinx's crash report prints
/// it: the message `PyObject_GenericGetAttr` raises, completed by the
/// suggestion `TracebackException` adds to an `AttributeError` that
/// carries its name and object ([`str_attribute_suggestion`]).
fn str_attribute_error(name: &str) -> SphinxCrash {
    let mut message = format!("'str' object has no attribute '{name}'");
    if let Some(suggestion) = str_attribute_suggestion(name) {
        message.push_str(&format!(". Did you mean: '{suggestion}'?"));
    }
    SphinxCrash::python("AttributeError", message, crate::rst::LogLocation::None)
}

/// `_compute_suggestion_error` (CPython 3.12 `Lib/traceback.py`) over
/// [`STR_ATTRIBUTES`]: of the names `str` has, in `dir` order, the first
/// closest to `wrong` by [`suggestion_distance`], provided no more than a
/// third of the characters involved change; none for a `wrong` longer
/// than 40 characters.
fn str_attribute_suggestion(wrong: &str) -> Option<&'static str> {
    let wrong: Vec<char> = wrong.chars().collect();
    // (`dir(obj)` past 750 names gives none: `str` has 81.)
    if wrong.len() > SUGGESTION_MAX_STRING_SIZE {
        return None;
    }
    let mut best_distance = wrong.len();
    let mut suggestion = None;
    // (`wrong` itself is never among them: `getattr` did not find it.)
    for possible in STR_ATTRIBUTES {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::py_repr_str;

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
        let outcome = |title: &str| match format_new_style(title, Some("Cap"), "1") {
            Ok(text) => format!("{text:?}"),
            Err(FormatError::KeyError(key)) => format!("KeyError: {}", py_repr_str(&key)),
            Err(FormatError::TypeError(crash) | FormatError::Crash(crash)) => crash.to_string(),
        };
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
            assert_eq!(outcome(title), expected, "{title:?}");
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
        let outcome = |title: &str| match format_new_style(title, Some("Cap"), "1") {
            Ok(text) => format!("{text:?}"),
            Err(FormatError::KeyError(key)) => format!("KeyError: {}", py_repr_str(&key)),
            Err(FormatError::TypeError(crash) | FormatError::Crash(crash)) => crash.to_string(),
        };
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
            assert_eq!(outcome(title), expected, "{title:?}");
        }
    }

    /// Not modelled (T19b's divergence list): a field's conversion and
    /// format spec, and the value of an attribute `str` has, each read as a
    /// `KeyError` of the whole field text, once the lookups before it
    /// succeeded. CPython 3.12 gives, in order: `'1'`, `  1`, `'1'`,
    /// `<built-in method upper of str object at 0x…>`, `TypeError:
    /// 'builtin_function_or_method' object is not subscriptable` and
    /// `AttributeError: 'builtin_function_or_method' object has no attribute
    /// 'x'`.
    #[test]
    fn format_fields_python_goes_on_with_read_as_a_key_error() {
        for title in [
            "{number!r}",
            "{number:>3}",
            "{number[0]!r}",
            "{number.upper}",
            "{number.upper[0]}",
            "{number.upper.x}",
        ] {
            let field = &title[1..title.len() - 1];
            assert!(
                matches!(
                    format_new_style(title, Some("Cap"), "1"),
                    Err(FormatError::KeyError(key)) if key == field
                ),
                "{title}"
            );
        }
    }
}
