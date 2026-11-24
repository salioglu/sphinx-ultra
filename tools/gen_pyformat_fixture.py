#!/usr/bin/env python3
"""Generate tests/fixtures/pyformat_units.json from CPython 3.12.2.

Regenerate with the locked core oracle profile:

    PYTHONNOUSERSITE=1 uv run --locked --python 3.12 \
        --project tools/oracle_profiles/core \
        python tools/gen_pyformat_fixture.py

Sphinx 9.1.0 formats three configuration values with Python's own string
formatting: `math_eqref_format.format(number=eqno)` at resolve time
(`SP/sphinx/domains/math.py:126-131`); a `numfig_format` entry or an explicit
`:numref:` title, with `title.format(name=figname, number=fignum)` or
`title.format(number=fignum)`, or with `title % fignum` when the title holds
neither `{name}` nor `number` (`SP/sphinx/domains/std/__init__.py:1132-1159`);
and `manpages_url.format_map(info)` at parse time (`SP/sphinx/roles.py:
531-551`), whose `section` is `None` when the target names none. Every value
is a `str` but that `section`. It also reads directive options with `int()`
(docutils), and `SOURCE_DATE_EPOCH` with `int()` and `float()`. src/pyformat.rs
ports all of it; this generator records what CPython returns or raises, row by
row, so the port is compared with CPython rather than with a reading of its C
source (`Objects/stringlib/unicode_format.h`, `Python/formatter_unicode.c`,
`PyUnicode_Format` in `Objects/unicodeobject.c`, `PyLong_FromUnicodeObject`,
`PyFloat_FromString`).

The fixture's sections:

- `format_args`, `format_map_args`, `percent_args`: the arguments the rows
  name. `format_args` sets are keyword arguments: `number` and `name` are
  `str`, and the `math` set has no `name`, as `MathDomain` passes none.
  `format_map_args` sets are manual-page `info` mappings, whose `section` is
  `None` (JSON `null`) in the `none` set.
- `format`: `fmt.format(**kwargs)`, every format of `FORMATS` under every set
  of `format_args`.
- `format_map`: `fmt.format_map(info)`, every format of `FORMAT_MAP_FORMATS`
  under every set of `format_map_args`.
- `percent`: `fmt % arg`, every format of `PERCENT_FORMATS` with every
  argument of `percent_args` (one `str`, neither a tuple nor a mapping).
- `random_format`, `random_format_map`, `random_percent`: formats drawn with
  `random.Random(seed)` from each section's alphabet (`*_ALPHABET`), each
  under the argument sets the section names (`args`), in order. A run of
  more than four digits is redrawn, so that no width asks for gigabytes.
- `int`, `float`: `int(text)` and `float(text)` for a `str`, in base 10. A
  row's `text` is a list of `[piece, count]` pairs, joined, so that the
  4,300-digit inputs stay readable; a float's value is its IEEE 754 bits.
- `code_points`: over every code point `c` but the surrogates, the inclusive
  runs where `int(c + "1" + c)` and `float(c + "1" + c)` succeed, and where
  they give 1: the whitespace both strip, and the decimal digits both read.
- `chr`: `chr(int(text, base))`, as docutils' `unicode_code` reads a
  character code, in base 10 and 16; a row's `text` is pieces as for `int`,
  and its outcome the code point `chr` gives (an int), or what it raises.
- `float_repr`: `repr(value)` of a float, given by its IEEE 754 bits: the
  hand-picked values of `FLOAT_REPR_VALUES`, then seeded random bit patterns
  and random decimals.

A row's outcome is `{"ok": text}`, or `{"raises": line}`, where `line` is
what `traceback.format_exception_only` prints for the exception, its class
and message: the last line of the traceback that Sphinx's crash report shows,
with the `Did you mean` suggestion Python adds to an `AttributeError`.

Two kinds of row are refused, not recorded:

- A format that looks up an attribute its value has (`{number.upper}`) before
  it raises or ends. Python goes on with the attribute's value, a bound
  method mostly, whose text names its address in memory; the port does not
  model it (T19b's divergence list), and the generator refuses such a format
  among the hand-written ones and skips it among the random ones.
- A value that `repr()` or `ascii()` renders, or that an error message
  quotes, holding a character of the categories Cf, Co or Cn, which
  `crate::utils::py_repr_str` does not escape (its ledgered gap). Every input
  is checked, so the gap touches no row.
"""

import _string
import json
import math
import random
import struct
import sys
import traceback
import unicodedata
from pathlib import Path

import docutils
import sphinx

EXPECTED_PYTHON = "3.12.2"
EXPECTED_DOCUTILS = "0.22.4"
EXPECTED_SPHINX = "9.1.0"
assert sys.version.split()[0] == EXPECTED_PYTHON, (
    f"Python {sys.version.split()[0]} != {EXPECTED_PYTHON}; run through the "
    "locked core profile (see the module docstring)"
)
assert docutils.__version__ == EXPECTED_DOCUTILS, (
    f"docutils {docutils.__version__} != {EXPECTED_DOCUTILS}; run through the "
    "locked core profile (see the module docstring)"
)
assert sphinx.__version__ == EXPECTED_SPHINX, (
    f"sphinx {sphinx.__version__} != {EXPECTED_SPHINX}; run through the "
    "locked core profile (see the module docstring)"
)
# Sphinx and docutils leave `int()`'s digit limit at its default.
assert sys.get_int_max_str_digits() == 4300, sys.get_int_max_str_digits()

REPO_ROOT = Path(__file__).resolve().parent.parent
FIXTURE_PATH = REPO_ROOT / "tests" / "fixtures" / "pyformat_units.json"

# The keyword arguments of `str.format`: `numref` as `_resolve_numref_xref`
# passes them with a caption, `math` as `MathDomain.resolve_xref` does (no
# `name`), and values that exercise `repr()`, `ascii()`, widths and
# precisions: a dotted number, an empty one (an equation the numbering did
# not reach), non-ASCII text of one, two and four bytes, both quotes, and the
# control and separator characters `repr()` escapes.
FORMAT_ARGS = {
    "numref": {"name": "Cap", "number": "1"},
    "math": {"number": "1"},
    "dotted": {"name": "A caption", "number": "1.2"},
    "empty": {"name": "", "number": ""},
    "wide": {"name": "Caf\u00e9 \u4e2d\u6587 \U0001f600", "number": "12"},
    "quotes": {"name": "it's \"q\"", "number": "a'b"},
    "escapes": {"name": "a\nb\tc\\d\x1f\x7f\u00a0\u2028\U0001f600", "number": "3"},
}

# The manual-page `info` mappings `Manpage.run` builds: `ls(1)` (a section),
# `ls.` (the regex matches with no section: `None`), and a target the regex
# does not match (`section` is `''`).
FORMAT_MAP_ARGS = {
    "section": {"page": "ls", "path": "ls(1)", "section": "1"},
    "none": {"page": "ls", "path": "ls.", "section": None},
    "empty": {"page": "printf", "path": "printf", "section": ""},
}

# The one `str` argument of `title % fignum`: a number, a dotted one, one
# character of each UTF-8 width, the empty string, both quotes and a newline.
PERCENT_ARGS = ["1", "1.2", "\u00e9", "\U0001f600", "", "it's \"q\"", "a\nb"]

FORMATS = [
    # Literal text, doubled braces, and the errors of `MarkupIterator`.
    "", "Fig.", "Eq {number}", "({number})", "{name} {number}", "{{", "}}",
    "{{number}}", "}}{{", "a{{", "{{x}", "a}", "}", "{", "a{", "{number}}",
    "{number", "{0", "{x[}", "{x!r", "{number:{", "{x!", "{x!rr}", "{ {",
    "{a{b}", "{number} {", "{nope} {", "{number}}}",
    # Field names: positional fields (an `IndexError`, no positional argument
    # is given), keywords, a `KeyError` for any other name, digits of any
    # script, and the overflow of `get_integer`.
    "{}", "{0}", "{00}", "{1}", "{number} {}", "{!r}", "{[0]}", "{.x}",
    "{\u0663}", "{1\u0660}", "{\U0001d7ce}", "{\u00b2}", "{-1}", "{x.}",
    "{x]}", "{number }", "{ number}", "{12x}", "{NUMBER}", "{name}",
    "{99999999999999999999}", "{922337203685477580x}",
    "{99999999999999999999x}", "{9223372036854775807}",
    "{9223372036854775808}",
    # The rest of a field name, looked up part by part: indexes, attributes
    # `str` lacks (with Python's suggestions), and the errors of
    # `FieldNameIterator`.
    "{number[0]}", "{number[00]}", "{name[1]}", "{name[0][0]}",
    "{number[1]}", "{name[2]}", "{name[\u0661]}", "{number[\u0660]}",
    "{name[\u0669\u0669]}", "{number[x]}", "{number[-1]}", "{number[ 0]}",
    "{number[0}]}", "{number[[]}", "{number[]}", "{number.}", "{number..}",
    "{number[0].}", "{number[0]x}", "{number[0]]}", "{number[9223372036854775807]}",
    "{number[9223372036854775808]}", "{number.real}", "{number.x}",
    "{number.uper}", "{number.Upper}", "{number.strp}", "{number.__lenn__}",
    "{number.formatmap}", "{name.lowr}", "{number.\u0130}",
    "{number.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}",
    # Conversions (item 1): `!s`, `!r` and `!a`, any other character a
    # `ValueError` (CPython's `\\x` form outside 33-126), a missing one, and
    # anything but `:` or `}` after it; applied before the format spec.
    "{number!s}", "{number!r}", "{number!a}", "{name!s}", "{name!r}",
    "{name!a}", "{number!r:>7}", "{name!a:*^30}", "{name!r:.3}",
    "{number!z}", "{number!R}", "{number!S}", "{number!\u00e9}",
    "{number! }", "{number!\x7f}", "{number!\x1f}", "{number!~}",
    "{number!\u4e2d}", "{number!\U0001f600}", "{number!}", "{number!}}",
    "{number!} x", "{number!rs}", "{number!!}", "{number!:}", "{number!r:}",
    "{number!r!}", "{nope!z}", "{number[x]!z}", "{number[5]!z}",
    "{number.x!z}", "{number!z:{0}}", "{number:{0}!z}",
    # Format specs on a `str` (item 2): fill and alignment, sign, `z`, `#`,
    # `0`, width and precision (digits of any script), `,` and `_`, the
    # presentation type, and the errors in CPython's order.
    "{number:}", "{number:<5}", "{number:>5}", "{number:^5}", "{number:^6}",
    "{name:^6}", "{name:^7}", "{name:*^8}", "{name:\u00e9>9}",
    "{name:\u4e2d^9}", "{name:\U0001f600<9}", "{number:x<}", "{number:<}",
    "{number:>}", "{number:^}", "{number:=5}", "{number:x=5}", "{number:z=}",
    "{number:=}", "{number:+}", "{number:-}", "{number: }", "{number:+5}",
    "{number:z}", "{number:+z}", "{number:z#}", "{number:#}", "{number:#5}",
    "{number:05}", "{number:<05}", "{number:x<05}", "{number:0>5}",
    "{number:^05}", "{number:0}", "{number:00}", "{number:000}",
    "{number:005}", "{number:05.1}", "{number:\u0665}", "{number:\u0660\u0665}",
    "{number:\U0001d7d3}", "{number:1\u0660}", "{number:\u00b2}",
    "{name:.2}", "{name:.0}", "{name:.\u0662}", "{name:5.2}", "{name:^5.2}",
    "{name:.99}", "{number:.}", "{number:.x}", "{number:5.}",
    "{number:.9223372036854775807}", "{number:.9223372036854775808}",
    "{number:9223372036854775808}", "{number:99999999999999999999}",
    "{number:,}", "{number:_}", "{number:,_}", "{number:_,}", "{number:,,}",
    "{number:__}", "{number:,d}", "{number:_x}", "{number:,x}", "{number:_b}",
    "{number:,s}", "{number:_\x80}", "{number:,\u00e9}", "{number:5,}",
    "{number:ss}", "{number:5ss}", "{number:>5ss}", "{name:\u00e9 \u00e9}",
    "{number:a\nb}", "{number:s}", "{number:5s}", "{number:>5.1s}",
    "{number:d}", "{number:x}", "{number:\u00e9}", "{number:\x7f}",
    "{number:\x80}", "{number:!}", "{number:\x1f}", "{number:\u4e2d}",
    "{number:\U0001f600}", "{number:5d}", "{number:+d}", "{number:#d}",
    "{number:=d}", "{number:+z#0,.3d}", "{number:=s}", "{number:+=s}",
    "{number:+s}", "{number: s}", "{number:zs}", "{number:#s}",
    "{number:%}", "{number:n}", "{number:c}",
    # Widths no allocation holds: `MemoryError`.
    "{number:9223372036854775807}", "{number:1000000000000000000}",
    "{number:>1000000000000000000}", "a{number:9223372036854775807}",
    # Nested fields in a spec (item 3), expanded first with the same
    # arguments; a recursion depth of 2; positional and automatic numbering
    # inside a spec as at the top level.
    "{number:{number}}", "{number:>{number}}", "{name:{number}}",
    "{number:{name}}", "{number:x>{name}}", "{number:{number}{number}}",
    "{number:{{}}}", "{number:{number:{number}}}", "{number:{}}",
    "{number:{0}}", "{number:{nope}}", "{nope:{0}}", "{number:{number!r}}",
    "{number:{number:{}}}", "{number:{name[0]}}", "{number:{number}x}",
    "{number:{number}:}", "{number:{}}{}", "{:{}}", "{number:{name.x}}",
    "{number:{number[5]}}", "{number:{number:>{number}}}",
    "{name:{number}.{number}}", "{number:{",
]

FORMAT_MAP_FORMATS = [
    # Manual-page URLs as projects write them, and the empty one.
    "", "https://man.example/{page}.{section}", "https://man.example/{path}",
    "https://manpages.debian.org/{path}", "{page}({section})",
    # `None` (item 4): an empty spec and `!s` give 'None', as do `!r` and
    # `!a`; any other spec is a `TypeError`; an attribute or an index on
    # `None` raises what Python raises.
    "{section}", "{section!s}", "{section!r}", "{section!a}", "{section:}",
    "{section:x}", "{section:>5}", "{section:s}", "{section!s:>5}",
    "{section!r:^8}", "{section!a:.2}", "{section.x}", "{section.real}",
    "{section.__clas__}", "{section.__bool}", "{section.__init_subclas__}",
    "{section.\u0130}", "{section[0]}", "{section[x]}", "{section[\u0660]}",
    "{section[0}", "{section!z}", "{section:{}}", "{section:{foo}}",
    "{section:{page}}", "{section:{section}}", "{page:{section}}",
    "{section!s:{section}}",
    # `format_map` takes no positional argument: a positional field is a
    # `ValueError`, raised where the field is looked up.
    "{}", "{0}", "{0[0]}", "{.x}", "{[0]}", "{page:{}}", "{page:{0}}",
    "{foo}", "{foo:{}}", "{} {foo}", "{foo} {}", "{\u0663}",
    # The parser and the lookups, as for `str.format`.
    "x{", "{a{b}", "x}", "{page", "{page!z}", "{page:>10}", "{page:.1}",
    "{path!r}", "{page.x}", "{page[9]}", "{page[0]}", "{PATH}",
    "{page!r:^9}", "{page:d}", "{page:,}", "{page:{page:{page}}}",
]

PERCENT_FORMATS = [
    # Sphinx's own old-style default, a title with no conversion, and the
    # inputs of the brief.
    "Fig. %s", "Figure 1", "Table %", "Tab %z", "100%", "100%%", "%%", "",
    # `s`, `r` and `a` render, honouring `-`, the width and the precision;
    # the other flags change nothing for a string.
    "%s", "%r", "%a", "%5s", "%-5s|", "%.1s", "%5.1s", "%-5.1s|", "%.0s",
    "%.s", "%+s", "% s", "%#s", "%05s", "%-05s|", "%-+ #0s", "%5r", "%.2r",
    "%-6a|", "%.0r", "%10.3a",
    # `c`: a one-character `str`, a `TypeError` otherwise.
    "%c", "%5c", "%-3c|", "%.0c", "%c%c",
    # The numeric conversions, each a `TypeError` for a `str`.
    "%d", "%i", "%u", "%o", "%x", "%X", "%e", "%E", "%f", "%F", "%g", "%G",
    "%5d", "%-d",
    # A mapping key, `*`, the length modifiers, an incomplete format, and
    # the argument fetched before the conversion character is read.
    "%(x)s", "%-(x)s", "%s%(x)s", "%*s", "%.*s", "%-*s", "%*%", "%5",
    "%-", "%.", "% ", "%5.", "%ls", "%hs", "%Ls", "%hhs", "%lls", "%l",
    "%h", "%L", "%s%", "%s%%", "a%sb%", "%%%s", "%%s", "%5%", "%s %s",
    "%s %z", "%z %s", "%s%z",
    # Unsupported characters, printed raw within 31-126 and as `?` outside.
    "%z", "%\x1f", "%\x1e", "%\x7f", "%\u00e9", "%\x80", "%\u0665s",
    "%.\u0662s", "\u00e9%z", "\U0001f600 %y", "%!", "%~", "%{", "%\n",
    # Widths and precisions: the overflow of each, and a width no allocation
    # holds (`MemoryError`).
    "%9223372036854775808s", "%9223372036854775807s", "%1000000000000000000s",
    "%.2147483648s", "%.2147483647s", "%99999999999999999999s",
]

INT_TEXTS = [
    "1", "0", "-0", "+1", "-1", " 1 ", "\t1\n", "00", "007", "1_000", "1_0_0",
    "1__0", "_1", "1_", "_", "", " ", "+", "-", "+-1", "- 1", "1 2", "1.5",
    "1e5", "0x10", "0b1", "0o7", "1j", "\u0661\u0662\u0663",
    "\uff11\uff17", "\u0661_\u0662", "\u00a01\u3000", "\u20281\u2029",
    "\x1c1", "1\x1f", "\x851\x85", "1\x00", "\u00b2", "\u2167", "1\u00e9",
    "-\u0661", "\u0661\u0662\u0663\u0664\u0665\u0666\u0667\u0668\u0669\u0660" * 3, "9" * 30, "-" + "9" * 30,
    "'" * 250, "\n" * 150, "a" * 250, "\u00e9" * 250, "x\"y'z",
]

# 4,300 digits is `int()`'s limit, leading zeros and digits of any script
# counted, underscores and the sign not; a literal it cannot read is reported
# first.
INT_PIECES = [
    [["1", 4300]],
    [["1", 4301]],
    [["0", 4300], ["1", 1]],
    [["0", 4300]],
    [["0", 4301]],
    [["1_", 2150], ["1", 1]],
    [["1_", 2149], ["1", 2]],
    [["-", 1], ["1", 4301]],
    [[" ", 3], ["1", 4301], [" ", 3]],
    [["\u0661", 4301]],
    [["1", 4301], ["x", 1]],
    [["1", 4301], ["_", 1]],
]

FLOAT_TEXTS = [
    "1", "1.5", " 1.5 ", "-1.5", "+1.5", "1e5", "1E5", "1e-5", "1e+5", "1e",
    "1e+", "1.", ".5", ".", "-.5", "1.e5", ".e5", "inf", "-inf", "+inf",
    "Infinity", "-INFINITY", "infinit", "infinityx", "in", "nan", "-nan",
    "+NaN", "nAn", "nan(1)", "1_000.5", "1__0", "_1", "1_", "1_.5", "1._5",
    "1e1_0", "1e_1", "0x10", "1e400", "-1e400", "1e-400", "-0", "-0.0",
    "1.7976931348623157e308", "1.7976931348623159e308", "2.5e-324",
    "4.9e-324", "0.1", "\u0661.\u0665", "\uff11\uff17", "\u00a01\u3000",
    "\x1c1", "1\x1f", "", " ", "1\x00", "1 2", "1.5.5", "1,5", "1700000000",
    "1700000000.123456789", "-67768040609740801", "9" * 400, "1" * 5000,
    "0." + "0" * 400 + "1", "1j", "\u00b2", "1\u00e9", "\u0661_\u0662.5",
    # The message names the text by its whole `repr()`, uncut.
    "x" * 250, "\u00e9" * 120 + "'",
]

FORMAT_ALPHABET = [
    "{", "}", "{", "}", "[", "]", "!", ":", ".", "<", ">", "^", "=", "+", "-",
    " ", "z", "#", "0", "1", "5", ",", "_", "s", "r", "a", "x", "\u0663",
    "\u00e9", "number", "name", "number", "{number}", "{name}", "0", "{}",
]
FORMAT_MAP_ALPHABET = [
    "{", "}", "{", "}", "[", "]", "!", ":", ".", "<", ">", "^", "+", " ",
    "0", "1", "5", "_", "s", "r", "a", "x", "page", "path", "section",
    "section", "{section}", "{page}", "0", "{}",
]
PERCENT_ALPHABET = [
    "%", "%", "%", "s", "r", "a", "c", "d", "z", "-", "+", " ", "#", "0",
    "1", "5", ".", "*", "(", ")", "l", "h", "x", "\u00e9", "%s", "%%",
]
RANDOM_FORMATS = 1000
RANDOM_FORMAT_MAPS = 400
RANDOM_PERCENTS = 600
# The argument sets the random formats run under: a caption and none, and
# text of every UTF-8 width; every manual-page `info`; a dotted number,
# non-ASCII text and the empty string.
RANDOM_FORMAT_ARGS = ["numref", "math", "wide"]
RANDOM_FORMAT_MAP_ARGS = ["section", "none", "empty"]
RANDOM_PERCENT_ARGS = ["1.2", "\u00e9", ""]

# docutils' `unicode_code` (`SP/docutils/parsers/rst/directives/__init__.py:
# 330-352`) reads a character code with `chr(int(code))` when
# `code.isdigit()`, and with `chr(int(value, 16))` for the `[0-9a-f]+` (any
# case) of a hexadecimal one, and words `chr`'s `OverflowError` as its own
# `ValueError`. CPython 3.12's `chr` takes a C `int`: past `INT_MAX` it is
# `OverflowError`, and from 0x110000 up to `INT_MAX` `ValueError`; a
# surrogate is a code point it gives. `int()` in base 16 has no digit limit.
CHR_TEXTS = [
    # Base 10: the boundaries 0x10FFFF, 0x110000, 2**31-1, 2**31, 2**32-1,
    # 2**32, 2**63-1, 2**63 and 2**64, leading zeros, the surrogates, decimal
    # digits of another script, and a digit `isdigit` admits and `int()`
    # does not.
    ("0", 10), ("65", 10), ("0065", 10), ("55296", 10), ("57343", 10),
    ("1114111", 10), ("1114112", 10), ("2147483647", 10), ("2147483648", 10),
    ("4294967295", 10), ("4294967296", 10), ("9223372036854775807", 10),
    ("9223372036854775808", 10), ("18446744073709551616", 10),
    ("00000000000000000000001114111", 10), ("\u0661\u0662", 10), ("\u00b2", 10),
    # Base 16: the same boundaries, both cases, leading zeros.
    ("0", 16), ("41", 16), ("d800", 16), ("DFFF", 16), ("10ffff", 16),
    ("10FFFF", 16), ("110000", 16), ("7fffffff", 16), ("80000000", 16),
    ("ffffffff", 16), ("100000000", 16), ("7FFFFFFFFFFFFFFF", 16),
    ("8000000000000000", 16), ("10000000000000000", 16),
    ("0000000000000000000010ffff", 16), ("000000000000000000000110000", 16),
]

# Codes of thousands of digits: `int()`'s limit holds in base 10 (leading
# zeros counted), not in base 16.
CHR_PIECES = [
    ([["1", 4300]], 10),
    ([["1", 4301]], 10),
    ([["0", 4293], ["1114111", 1]], 10),
    ([["1", 4301]], 16),
    ([["0", 5000], ["41", 1]], 16),
]

# Floats as docutils' `get_measure` prints a length `float()` reads
# (`f'{value}{unit}'`, `repr()`), and as `ast.unparse` prints a float
# constant: the shortest digits that read back as the value, in exponent form
# when the decimal exponent is below -4 or at least 16.
FLOAT_REPR_VALUES = [
    0.0, -0.0, 1.0, -1.0, 2.0, 10.0, 100.0, 0.5, 0.1, 0.2, 0.1 + 0.2, 1 / 3,
    2 / 3, 2.675, 4.35, 1.1, 1e-7, 1e7,
    # Tiny and huge: the least subnormal, the least normal, the greatest.
    5e-324, 1e-323, 2.2250738585072014e-308, 2.225073858507201e-308, 1e-300,
    1e-100, 1e100, 1e300, 1.7976931348623157e308,
    # Integral values.
    1e15, 1e16, 1e17, 1e22, 1e23, 2.0 ** 53, 2.0 ** 53 + 2, 9007199254740993.0,
    123456789012345678.0, 9999999999999998.0, 1234567890123456.0,
    12345678901234567.0, 4503599627370496.5,
    # The decimal exponents -5, -4, 15, 16 and 17.
    1.5e-5, 9.99e-5, 1.5e-4, 0.00012345678901234567, 1.5e15, 1.5e16, 1.5e17,
    # Seventeen significant digits.
    12345678901234567.5, 1.2345678901234567, 123456789.12345678,
    1234567890123456.7, 0.1234567890123456789, 9.876543210987654e-5,
    # Negative values, the infinities and NaN.
    -1.5, -1e-5, -1e16, -12345678901234567.5, -5e-324,
    float("inf"), float("-inf"), float("nan"), -float("nan"),
]
# Each of these exponents, the float of 1eN and its two neighbours.
FLOAT_REPR_EXPONENTS = [-5, -4, 15, 16, 17]
FLOAT_REPR_RANDOM_BITS = 1000
FLOAT_REPR_RANDOM_DECIMALS = 1000


def exception_line(exc):
    """What `traceback.format_exception_only` prints, as Sphinx's crash
    report shows the traceback's last line."""
    return "".join(traceback.format_exception_only(type(exc), exc)).rstrip("\n")


def outcome(call):
    """`{"ok": text}`, or the exception `call` raises (`MemoryError`
    included, which is an `Exception`)."""
    try:
        result = call()
    except Exception as exc:  # noqa: BLE001 - every exception is recorded
        return {"raises": exception_line(exc)}
    assert not isinstance(result, str) or " at 0x" not in result, result
    return {"ok": result}


def reaches_attribute(fmt, lookup, depth=2):
    """Whether `str.format` reaches an attribute its value has before it
    raises or ends, walked with CPython's own parser (`_string`) and the
    real values. It may say yes for a format Python stops in earlier; it
    never says no for one that reaches such an attribute."""
    if depth <= 0:
        return False
    try:
        for _literal, field, spec, conversion in _string.formatter_parser(fmt):
            if field is None:
                continue
            first, rest = _string.formatter_field_name_split(field)
            try:
                value = lookup(first)
            except Exception:  # noqa: BLE001
                return False
            for is_attribute, key in rest:
                if is_attribute:
                    if hasattr(value, key):
                        return True
                    return False
                try:
                    value = value[key]
                except Exception:  # noqa: BLE001
                    return False
            if conversion not in (None, "r", "s", "a"):
                return False
            if spec and "{" in spec and reaches_attribute(spec, lookup, depth - 1):
                return True
    except Exception:  # noqa: BLE001
        return False
    return False


def keyword_lookup(kwargs):
    def lookup(first):
        if isinstance(first, int):
            raise IndexError(first)
        return kwargs[first]
    return lookup


def mapping_lookup(info):
    def lookup(first):
        if isinstance(first, int):
            raise ValueError(first)
        return info[first]
    return lookup


def repr_gap(text):
    """Whether `text` holds a character of the categories Cf, Co or Cn,
    which `py_repr_str` does not escape."""
    return any(unicodedata.category(c) in ("Cf", "Co", "Cn") for c in text)


def check_inputs(*texts):
    for text in texts:
        if text is not None and repr_gap(text):
            raise SystemExit(f"{text!r} holds a Cf, Co or Cn character (py_repr_str's gap)")


def long_digit_run(text):
    run = 0
    for c in text:
        run = run + 1 if unicodedata.decimal(c, None) is not None else 0
        if run > 4:
            return True
    return False


def format_rows(formats, arg_sets, call, lookup, refuse):
    rows = []
    for fmt in formats:
        check_inputs(fmt)
        for name, args in arg_sets.items():
            if reaches_attribute(fmt, lookup(args)):
                if refuse:
                    raise SystemExit(f"{fmt!r} under {name!r} reaches an attribute its value has")
                continue
            rows.append({"args": name, "format": fmt, **outcome(lambda: call(fmt, args))})
    return rows


def random_formats(seed, alphabet, count, arg_sets, call, lookup):
    """`count` formats drawn from `alphabet`, each with its outcome under
    every set of `arg_sets`, less those that reach an attribute their value
    has."""
    rng = random.Random(seed)
    rows = []
    drawn = 0
    while drawn < count:
        fmt = "".join(rng.choice(alphabet) for _ in range(rng.randint(1, 10)))
        if long_digit_run(fmt):
            continue
        drawn += 1
        if any(reaches_attribute(fmt, lookup(args)) for args in arg_sets):
            continue
        rows.append({"format": fmt, "out": [outcome(lambda: call(fmt, args)) for args in arg_sets]})
    return rows


def percent_rows():
    rows = []
    for fmt in PERCENT_FORMATS:
        check_inputs(fmt)
        for arg in PERCENT_ARGS:
            rows.append({"arg": arg, "format": fmt, **outcome(lambda: fmt % arg)})
    return rows


def random_percents(seed, count):
    """`count` formats drawn from `PERCENT_ALPHABET`, each with its outcome
    for every argument of `RANDOM_PERCENT_ARGS`."""
    rng = random.Random(seed)
    rows = []
    while len(rows) < count:
        fmt = "".join(rng.choice(PERCENT_ALPHABET) for _ in range(rng.randint(1, 8)))
        if long_digit_run(fmt):
            continue
        rows.append({"format": fmt, "out": [outcome(lambda: fmt % arg) for arg in RANDOM_PERCENT_ARGS]})
    return rows


def joined(pieces):
    return "".join(piece * count for piece, count in pieces)


def float_outcome(text):
    out = outcome(lambda: float(text))
    if "ok" in out:
        value = out["ok"]
        out = {"ok": repr(value), "bits": f"{struct.unpack('<Q', struct.pack('<d', value))[0]:016x}"}
    return out


def runs(points):
    out = []
    for point in points:
        if out and out[-1][1] == point - 1:
            out[-1][1] = point
        else:
            out.append([point, point])
    return out


def code_points():
    int_ok, int_one, float_ok, float_one = [], [], [], []
    for point in range(0x110000):
        if 0xD800 <= point <= 0xDFFF:
            continue
        c = chr(point)
        text = c + "1" + c
        try:
            value = int(text)
            int_ok.append(point)
            if value == 1:
                int_one.append(point)
        except ValueError:
            pass
        try:
            value = float(text)
            float_ok.append(point)
            if value == 1.0:
                float_one.append(point)
        except ValueError:
            pass
    return {
        "int_ok": runs(int_ok),
        "int_one": runs(int_one),
        "float_ok": runs(float_ok),
        "float_one": runs(float_one),
    }


def chr_rows():
    rows = []
    for pieces, base in [([[text, 1]], base) for text, base in CHR_TEXTS] + CHR_PIECES:
        text = joined(pieces)
        check_inputs(text)
        rows.append({"base": base, "text": pieces, **outcome(lambda: ord(chr(int(text, base))))})
    return rows


def float_bits(value):
    return f"{struct.unpack('<Q', struct.pack('<d', value))[0]:016x}"


def float_repr_rows():
    """`repr(value)` for the values of `FLOAT_REPR_VALUES`, 1eN and its two
    neighbours for each N of `FLOAT_REPR_EXPONENTS`, then random bit patterns
    and random decimals of one to seventeen digits, drawn with
    `random.Random(3)`."""
    values = list(FLOAT_REPR_VALUES)
    for exponent in FLOAT_REPR_EXPONENTS:
        value = float(f"1e{exponent}")
        values += [math.nextafter(value, 0.0), value, math.nextafter(value, math.inf)]
    rng = random.Random(3)
    for _ in range(FLOAT_REPR_RANDOM_BITS):
        values.append(struct.unpack("<d", struct.pack("<Q", rng.getrandbits(64)))[0])
    for _ in range(FLOAT_REPR_RANDOM_DECIMALS):
        digits = rng.randint(1, 17)
        mantissa = rng.randrange(10 ** (digits - 1), 10 ** digits)
        values.append(float(f"{'-' if rng.random() < 0.25 else ''}{mantissa}e{rng.randint(-30, 30)}"))
    return [{"bits": float_bits(value), "ok": repr(value)} for value in values]


def main():
    for args in FORMAT_ARGS.values():
        check_inputs(*args.values())
    for args in FORMAT_MAP_ARGS.values():
        check_inputs(*args.values())
    check_inputs(*PERCENT_ARGS, *FORMAT_ALPHABET, *FORMAT_MAP_ALPHABET, *PERCENT_ALPHABET)
    fixture = {
        "generator": "tools/gen_pyformat_fixture.py",
        "docutils_version": docutils.__version__,
        "sphinx_version": sphinx.__version__,
        "python_version": sys.version.split()[0],
        "unicodedata_version": unicodedata.unidata_version,
        "int_max_str_digits": sys.get_int_max_str_digits(),
        "format_args": FORMAT_ARGS,
        "format_map_args": FORMAT_MAP_ARGS,
        "percent_args": PERCENT_ARGS,
        "format": format_rows(
            FORMATS, FORMAT_ARGS, lambda fmt, args: fmt.format(**args), keyword_lookup, refuse=True
        ),
        "format_map": format_rows(
            FORMAT_MAP_FORMATS, FORMAT_MAP_ARGS, lambda fmt, info: fmt.format_map(info), mapping_lookup, refuse=True
        ),
        "percent": percent_rows(),
        "random_format": {
            "seed": 0,
            "alphabet": FORMAT_ALPHABET,
            "args": RANDOM_FORMAT_ARGS,
            "rows": random_formats(
                0,
                FORMAT_ALPHABET,
                RANDOM_FORMATS,
                [FORMAT_ARGS[name] for name in RANDOM_FORMAT_ARGS],
                lambda fmt, args: fmt.format(**args),
                keyword_lookup,
            ),
        },
        "random_format_map": {
            "seed": 1,
            "alphabet": FORMAT_MAP_ALPHABET,
            "args": RANDOM_FORMAT_MAP_ARGS,
            "rows": random_formats(
                1,
                FORMAT_MAP_ALPHABET,
                RANDOM_FORMAT_MAPS,
                [FORMAT_MAP_ARGS[name] for name in RANDOM_FORMAT_MAP_ARGS],
                lambda fmt, info: fmt.format_map(info),
                mapping_lookup,
            ),
        },
        "random_percent": {
            "seed": 2,
            "alphabet": PERCENT_ALPHABET,
            "args": RANDOM_PERCENT_ARGS,
            "rows": random_percents(2, RANDOM_PERCENTS),
        },
        "int": [],
        "float": [],
        "code_points": code_points(),
    }
    for text in INT_TEXTS:
        check_inputs(text)
        fixture["int"].append({"text": [[text, 1]], **outcome(lambda: str(int(text)))})
    for pieces in INT_PIECES:
        text = joined(pieces)
        fixture["int"].append({"text": pieces, **outcome(lambda: str(int(text)))})
    for text in FLOAT_TEXTS:
        check_inputs(text)
        fixture["float"].append({"text": [[text, 1]], **float_outcome(text)})
    fixture["chr"] = chr_rows()
    fixture["float_repr"] = float_repr_rows()
    FIXTURE_PATH.write_text(dump(fixture) + "\n", encoding="utf-8")
    random_rows = sum(len(fixture[key]["rows"]) for key in ("random_format", "random_format_map", "random_percent"))
    print(
        f"wrote {FIXTURE_PATH}: {len(fixture['format'])} format rows, "
        f"{len(fixture['format_map'])} format_map rows, {len(fixture['percent'])} percent rows, "
        f"{random_rows} random formats, {len(fixture['int'])} int rows, {len(fixture['float'])} float rows, "
        f"{len(fixture['chr'])} chr rows, {len(fixture['float_repr'])} float_repr rows"
    )


def dump(value, indent=0):
    """JSON with one line per row: the top levels indented, each element of
    a list of objects on a line of its own, so that a regeneration diffs row
    by row. ASCII only, so that no editor turns a no-break space into a
    space."""
    pad = "  " * indent
    if isinstance(value, dict) and indent < 2:
        items = [f"{pad}  {json.dumps(k)}: {dump(v, indent + 1)}" for k, v in sorted(value.items())]
        return "{\n" + ",\n".join(items) + f"\n{pad}}}"
    if isinstance(value, list) and value and all(isinstance(v, dict) for v in value):
        items = [f"{pad}  {json.dumps(v, sort_keys=True)}" for v in value]
        return "[\n" + ",\n".join(items) + f"\n{pad}]"
    return json.dumps(value, sort_keys=True)


if __name__ == "__main__":
    main()
