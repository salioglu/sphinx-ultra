#!/usr/bin/env python3
"""Generate the smart-quotes fixtures and tables from docutils 0.22.4 and
Sphinx 9.1.0.

Regenerate with the locked core oracle profile:

    PYTHONNOUSERSITE=1 uv run --locked --python 3.12 \
        --project tools/oracle_profiles/core \
        python tools/gen_smartquotes_fixture.py --units
    PYTHONNOUSERSITE=1 uv run --locked --python 3.12 \
        --project tools/oracle_profiles/core \
        python tools/gen_smartquotes_fixture.py --tables
    cargo fmt --all
    PYTHONNOUSERSITE=1 uv run --locked --python 3.12 \
        --project tools/oracle_profiles/core \
        python tools/gen_smartquotes_fixture.py --variant

The `cargo fmt` is not optional after `--tables`: rustfmt lays out the
emitted arrays, so skipping it leaves a diff that is pure formatting.

Sphinx 9.1.0 educates quotes, dashes and ellipses with docutils' SmartyPants
port, `docutils.utils.smartquotes` (`educate_tokens`, `SP/docutils/utils/
smartquotes.py:565-675`), fed by `SphinxSmartQuotes.get_tokens`
(`SP/sphinx/transforms/__init__.py:403-415`). src/transforms/smartquotes/
educate.rs ports those pure functions; this generator records what the
originals return, so the port is compared with them rather than with a
reading of their regular expressions (research report "smart quotes" §6.1,
§7.3-7.4).

--units   writes tests/fixtures/smartquotes_units.json:
            - `educate_tokens`: every input of the report's unit probe
              (`probe_units.py`, its Appendix A): the one-token rows under
              Sphinx's action `qDe` in each probe language, the action
              table (one input, the 16 actions), the multi-token context
              table, and the action-`0` row;
            - `educate_quotes`: `educateQuotes` called directly, where the
              first character is not the context character
              `educate_tokens` puts there, which is the only way to reach
              its `START_SINGLE`/`START_DOUBLE` passes;
            - `random`: 2,000 strings drawn with `random.Random(0)` from the
              report's alphabet, each one plain token, educated with the 16
              actions under `en` and with `qDe` under the languages whose
              quotes take another path (`RANDOM_LANGUAGES`);
            - `random_context`: 500 sequences of 2-4 tokens, plain or
              literal, drawn with `random.Random(1)`, educated with `qDe`
              under `en` and those languages: the context one token hands
              the next. In both random sections an output equal to its
              input is recorded as `null`, which halves the file;
            - `normalize_language_tag` with the tag docutils picks from
              `smartchars.quotes`, and `quote_sets`: `smartchars(language)`
              for every table key, cased variants and unknown tags;
            - `escapes`: `processEscapes` and its restore, and the
              backslashes Sphinx's `get_tokens` inserts after `\\x00`,
              taken from a real `SphinxSmartQuotes.get_tokens` call;
            - `char_classes`: the character classes the regular
              expressions use, each checked here against the compiled
              patterns over every code point, and the runs of `\\w` that
              `--tables` writes.
--tables  writes src/transforms/smartquotes/tables.rs: `smartchars.quotes`
          (the 75 languages docutils has quotes for) and the runs of
          Python's `\\w` (`str.isalnum()` or `_`), which the port's
          `APOSTROPHE`, `ADJACENT_*` and `OPENING_*` passes test. The runs
          live there, not in src/rst/digits.rs, which
          tools/gen_digit_tables.py alone writes.
--variant writes tests/fixtures/sphinx_smartquotes_differential.json: the
          snippet corpus of tools/gen_sphinx_fixture.py (its `CASES`, each
          with its own `conf`), read through that generator's harness,
          imported by path, twice: with `smartquotes = False`, which must
          give tests/fixtures/sphinx_doctree_differential.json's trees byte
          for byte (regenerate that first after a `CASES` change), and with
          `smartquotes = True`, both with `keep_warnings = True` and the
          dummy builder (report §6.3). A case is recorded when smart quotes
          change its tree or its warnings: both trees, and the warning lines
          only the second read printed, which must all be SmartQuotes' own
          `No smart quotes defined for language` records.
"""

import argparse
import importlib.util
import json
import os
import random
import re
import shutil
import sys
import tempfile
import unicodedata
from collections import Counter
from pathlib import Path

import docutils
import sphinx
from docutils import nodes
from docutils.utils import normalize_language_tag
from docutils.utils import smartquotes as sq
from sphinx.transforms import SphinxSmartQuotes
from sphinx.util.docutils import docutils_namespace, patch_docutils

EXPECTED_DOCUTILS = "0.22.4"
EXPECTED_SPHINX = "9.1.0"
assert docutils.__version__ == EXPECTED_DOCUTILS, (
    f"docutils {docutils.__version__} != {EXPECTED_DOCUTILS}; run through the "
    "locked core profile (see the module docstring)"
)
assert sphinx.__version__ == EXPECTED_SPHINX, (
    f"sphinx {sphinx.__version__} != {EXPECTED_SPHINX}; run through the "
    "locked core profile (see the module docstring)"
)

REPO_ROOT = Path(__file__).resolve().parent.parent
UNITS_PATH = REPO_ROOT / "tests" / "fixtures" / "smartquotes_units.json"
TABLES_PATH = REPO_ROOT / "src" / "transforms" / "smartquotes" / "tables.rs"
VARIANT_PATH = REPO_ROOT / "tests" / "fixtures" / "sphinx_smartquotes_differential.json"
BASE_FIXTURE = "tests/fixtures/sphinx_doctree_differential.json"
SPHINX_GENERATOR = REPO_ROOT / "tools" / "gen_sphinx_fixture.py"

# The one record SmartQuotes makes (`SP/docutils/transforms/universal.py:
# 324-328`): every warning line the smart-quotes read adds must be one.
SMARTQUOTES_WARNING = "WARNING: No smart quotes defined for language "

# Anti-truncation floor for the variant: 63 cases when --variant was added
# (wave 5a T16b). The corpus only grows, so a regeneration below the floor
# lost cases.
VARIANT_FLOOR = 55

# The report's action table (§S3): docutils' numbered presets, the letters
# one at a time and combined, and the strings that do nothing but run
# processEscapes.
ACTIONS = ["qDe", "q", "D", "d", "i", "e", "b", "B", "qw", "qBDe", "1", "2", "3", "0", "-1", ""]

# The report's probe languages (§6.1), each with something of its own: `de`
# opens low, `de-ch` and `fr-x-altquot` use guillemets, `fr` puts a
# no-break space inside them, `ja` uses corner brackets, `he` and `sv`
# close with what other languages open with, `sv`'s closing secondary
# quote is the apostrophe (so `APOSTROPHE` never runs), and `''` has no
# entry (ASCII quotes). Two more: `en-uk-x-altquot` swaps primary and
# secondary quotes, and `EN` finds `en`'s quotes but not the `DECADE`
# pass, which tests the case as given.
LANGUAGES = ["en", "de", "de-ch", "fr", "fr-x-altquot", "ja", "he", "sv", "", "en-uk-x-altquot", "EN"]

# `probe_units.py`'s one-token inputs (report Appendix A), verbatim.
ONE_TOKEN = [
    '"quoted"', "'single'", "it's", "80's", "the '80s", "'80s", "1960's",
    "a -- b", "a --- b", "a ---- b", "a ----- b", "a-b", "wait...", "wait....",
    "a . . . b", "a. . . .b",
    '"nested \'inner\' quotes"', "'nested \"inner\" quotes'",
    '"\'Quoted\' words in a larger quote."',
    '\'"Quoted" words\'',
    '("parens")', "('parens')", '["brackets"]', '{"braces"}',
    '-"dash"', "--'dash'",
    'x"y"z', "x'y'z", 'end"', "end'", '"', "'", '""', "''",
    '" spaced "', "' spaced '",
    '"Isn\'t this fun?"', "rock 'n' roll", "'tis", "dogs' bones",
    'He said, "Hi."', "She said, 'Hi.'",
    '"."', "'.'", '"!"', '"\'"',
    '``backtick\'\'', "`single'",
    'a &quot;b&quot;', '<b>"tag"</b>',
    '1\'2"', "5' 10\"", "O'Neil", "rock'n'roll",
    '"\u00e9t\u00e9"', "l'\u00e9t\u00e9",
    'tab\t"x"', 'nl\n"x"', '\u00a0"nbsp"', '\u200b"zwsp"',
    '\u2014"emdash"', '\u2013\'endash\'',
    '"a" "b"', "'a' 'b'",
    '"?"', '"#"', "'#'", '"x', "'x", 'x"', "x'",
    # escapes as get_tokens produces them (backslash inserted after \x00)
    '\x00\\"esc\x00\\"', "\x00\\'esc\x00\\'", 'a\x00\\--b', 'a\x00\\.\x00\\.\x00\\.',
    '\x00\\\\"x"',  # escaped backslash then quote
    'x\x00 "y"',    # escaped space before a quote
    "'\x00a'",      # escaped letter after a quote
]

# The action table's input (§S3).
ACTION_INPUT = '"Hi" -- it\'s --- ``x\'\' `y\' ... . . . &quot;z&quot;'

# The action-'0' probe (§S3): literal backslash, entity, already-curly text.
ACTION0_INPUT = 'a\\"b &#34; \u201cc\u201d \u2013 \u2026'

# `probe_units.py`'s context table (§S7), verbatim.
CONTEXT_SEQUENCES = [
    [("plain", "say "), ("plain", '"hi"')],
    [("plain", "say"), ("plain", '"hi"')],
    [("plain", "x"), ("literal", "code"), ("plain", '"hi"')],
    [("plain", "x "), ("literal", "code"), ("plain", '"hi"')],
    [("plain", "x "), ("literal", "code "), ("plain", '"hi"')],
    [("plain", '"'), ("plain", "hi"), ("plain", '"')],
    [("plain", "'"), ("plain", "hi"), ("plain", "'")],
    [("plain", "a"), ("plain", ""), ("plain", '"b"')],
    [("plain", "a "), ("literal", ""), ("plain", '"b"')],
    [("plain", "-"), ("plain", "-")],
    [("plain", "."), ("plain", "..")],
    [("plain", "("), ("plain", '"x")')],
    [("plain", 'a"'), ("plain", '"b')],
    [("plain", 'a\x00\\"'), ("plain", '"b')],
    [("literal", '"'), ("plain", 'x"')],
    [("plain", "end."), ("plain", '"Next" para')],
]

# Inputs only `educateQuotes` called directly can see: the first character
# is a quote, so the `START_*` passes (a quote, punctuation, then a literal
# backslash and `B`, the pattern text `\\B` read as a raw string) and the
# `^`-anchored lookbehinds have something to match.
EDUCATE_QUOTES_EXTRA = [
    '"!\\B', "'.\\B", '"x\\B', "'\\\\B", '" \\B', "'-\\Bx", '"\'\\B',
    '"', "'", '"x"', "'x'", "' x", '" x', "'\x00", '"\u00a0',
]

# The report's random alphabet (§6.1): every character a pass tests, the
# escape marker and backslash, a word character, a digit and `s` for
# `DECADE`, the opening brackets, `;` (the context a quote becomes), `&`
# and `#34` (an entity's pieces), and three non-ASCII separators.
ALPHABET = ['"', "'", "-", ".", "`", "\\", "\x00", " ", "x", "9", "s", "(", "[", "{", ";", "&", "#34", "\u00a0", "\u2013", "\u200b"]
RANDOM_STRINGS = 2000
RANDOM_SEQUENCES = 500

# The probe languages whose quotes take a different path through
# `educateQuotes` than `en`'s, for the random rows: `''` (ASCII quotes, which
# every later pass sees again), `fr` (two-character quotes whose no-break
# space is `\s` to the later passes), `sv` (its closing secondary quote is
# the apostrophe, so `APOSTROPHE` never runs) and `EN` (no `DECADE`).
RANDOM_LANGUAGES = ["", "fr", "sv", "EN"]

LANGUAGE_TAGS = [
    # §S4's 13 tags
    "en", "EN", "en_US", "en-GB", "de_AT-1901", "de-CH-x_altquot", "sr_Latn",
    "zh_CN", "zh-TW", "pt_BR", "xx", "a_b_c", "fr-x-altquot",
    # singletons, duplicates, the empty tag, a non-ASCII letter
    "", "x-a-b", "a-x-b-c", "en-us-us", "de_AT_1901_x_y", "-en", "en-", "EN-UK-x-altquot",
    "a_1_b_2_c", "\u00e9-fr",
]


def educate(tokens, attr, language):
    return list(sq.educate_tokens(tokens, attr=attr, language=language))


def random_string(rng):
    return "".join(rng.choice(ALPHABET) for _ in range(rng.randint(1, 12)))


def code_points(pred):
    return [cp for cp in range(0x110000) if pred(chr(cp))]


def runs(points):
    out = []
    for cp in points:
        if out and out[-1][1] == cp - 1:
            out[-1][1] = cp
        else:
            out.append([cp, cp])
    return out


def char_classes():
    """The classes of `RegularExpressions._CH_CLASSES` (`smartquotes.py:
    510-515`) and Python's `\\s`/`\\d`, each read off the compiled patterns
    over every code point, so the fixture holds what the patterns match
    rather than what their source seems to say."""
    regexes = sq.regexes
    full = sq.RegularExpressions._CH_CLASSES

    def members(cls):
        pattern = re.compile(cls)
        return "".join(chr(cp) for cp in range(0x110000) if pattern.fullmatch(chr(cp)))

    punct = members(full["punct"])
    open_ = members(full["open"])
    dash = members(full["dash"])
    sep = members(full["sep"])
    space = members(r"\s")
    word = code_points(lambda c: re.fullmatch(r"\w", c) is not None)
    decimal = code_points(lambda c: re.fullmatch(r"\d", c) is not None)
    assert space == "".join(chr(cp) for cp in code_points(str.isspace))
    assert word == code_points(lambda c: c.isalnum() or c == "_")
    assert decimal == code_points(lambda c: unicodedata.decimal(c, None) is not None)
    assert set(punct) <= {chr(cp) for cp in range(0x80)} and " " in punct and "&" not in punct
    # The compiled patterns use the classes as `_CH_CLASSES` spells them,
    # in VERBOSE mode too: what precedes and what follows the quote.
    for cp in range(0x110000):
        c = chr(cp)
        before = c in sep or c in open_ or c in dash
        assert bool(regexes.OPENING_PRIMARY.match(c + '"x')) == before, hex(cp)
        assert bool(regexes.OPENING_SECONDARY.match(c + "'x")) == before, hex(cp)
        after = re.fullmatch(r"\w", c) is not None or c in punct
        assert bool(regexes.OPENING_PRIMARY.match(' "' + c)) == after, hex(cp)
        assert bool(regexes.OPENING_SECONDARY.match(" '" + c)) == after, hex(cp)
        assert bool(regexes.OPEN_DOUBLE.match(c + '" ')) == (c in open_ or c in dash), hex(cp)
        assert bool(regexes.CLOSING_SECONDARY.search("a" + c + "'")) == (c not in space), hex(cp)
    return {
        "punct": punct,
        "open": open_,
        "dash": dash,
        "sep": sep,
        "space": space,
        "decimal_runs": runs(decimal),
        "word_runs": runs(word),
    }


def get_tokens_plain(data):
    """What Sphinx's `get_tokens` yields for a `Text(data)` in a paragraph."""
    paragraph = nodes.paragraph()
    text = nodes.Text(data)
    paragraph += text
    ((kind, out),) = SphinxSmartQuotes.get_tokens(None, [text])
    assert kind == "plain", kind
    return out


def units():
    educate_rows = []
    for language in LANGUAGES:
        for text in ONE_TOKEN:
            tokens = [("plain", text)]
            educate_rows.append({"attr": "qDe", "language": language, "tokens": tokens, "out": educate(tokens, "qDe", language)})
    for attr in ACTIONS:
        for text in (ACTION_INPUT, ACTION0_INPUT):
            tokens = [("plain", text)]
            educate_rows.append({"attr": attr, "language": "en", "tokens": tokens, "out": educate(tokens, attr, "en")})
    for language in LANGUAGES:
        for tokens in CONTEXT_SEQUENCES:
            educate_rows.append({"attr": "qDe", "language": language, "tokens": tokens, "out": educate(tokens, "qDe", language)})

    quote_rows = []
    for language in LANGUAGES:
        for text in ONE_TOKEN + EDUCATE_QUOTES_EXTRA:
            quote_rows.append({"language": language, "text": text, "out": sq.educateQuotes(text, language)})

    random_settings = [[attr, "en"] for attr in ACTIONS] + [["qDe", language] for language in RANDOM_LANGUAGES]
    rng = random.Random(0)
    random_rows = []
    for _ in range(RANDOM_STRINGS):
        text = random_string(rng)
        out = [educate([("plain", text)], attr, language)[0] for attr, language in random_settings]
        random_rows.append({"tokens": [("plain", text)], "out": [None if o == text else o for o in out]})

    context_settings = [["qDe", "en"]] + [["qDe", language] for language in RANDOM_LANGUAGES]
    rng = random.Random(1)
    context_rows = []
    for _ in range(RANDOM_SEQUENCES):
        tokens = []
        for _ in range(rng.randint(2, 4)):
            kind = "literal" if rng.random() < 0.25 else "plain"
            text = "" if rng.random() < 0.1 else random_string(rng)
            tokens.append((kind, text))
        texts = [text for _, text in tokens]
        out = [educate(tokens, attr, language) for attr, language in context_settings]
        context_rows.append({"tokens": tokens, "out": [None if o == texts else o for o in out]})

    tag_rows = []
    for tag in LANGUAGE_TAGS:
        tags = normalize_language_tag(tag)
        pick = next((t for t in tags if t in sq.smartchars.quotes), None)
        tag_rows.append({"tag": tag, "tags": tags, "quotes": pick})

    quote_sets = []
    keys = sorted(sq.smartchars.quotes)
    probes = keys + [k.upper() for k in keys if "-" in k] + ["En", "", "xx", "en_us", "en-us", "fr_x_altquot", "zh"]
    for language in probes:
        smart = sq.smartchars(language)
        quote_sets.append({
            "language": language,
            "known": language in sq.smartchars.quotes,
            "quotes": [smart.opquote, smart.cpquote, smart.osquote, smart.csquote],
        })

    escape_inputs = [
        '\\\\ \\" \\\' \\. \\- \\` \\x', "&#92; &#34; &#39; &#46; &#45; &#96; &#35;",
        '\\\\"', "\\\\-", "&#92;#34;", "&&#34;;", '\x00\\"esc\x00\\"', "a\x00\\.\x00\\.",
        "\\", "", "&#3", "\\&#96;", "&#92;&#96;",
    ]
    escapes = [
        {"text": text, "escaped": sq.processEscapes(text), "restored": sq.processEscapes(text, restore=True)}
        for text in escape_inputs
    ]
    backslash_inputs = [
        '\x00"escaped double\x00" and \x00\'escaped single\x00\' and it\x00\'s.',
        "Escaped dashes \x00-\x00- and \x00-- and ellipsis \x00.\x00.\x00. and \x00... done.",
        "Backslash then quote \x00\\\u201dx\u201d and escaped space x\x00 \u201cy\u201d and \u2018\x00a\u2019.",
        "Escaped letter \u201c\x00a\u201d and \x00`backtick\x00` done.",
        "\x00\x00\"", "\x00\\\x00\\", "\x00", "a\x00", "\x00-\x00-\x00-", "no markers \\\" here",
        "\x00x\x00 \x00\n\x00\u2013",
    ]
    inserted = [{"data": data, "out": get_tokens_plain(data)} for data in backslash_inputs]

    return {
        "generator": "tools/gen_smartquotes_fixture.py --units",
        "docutils_version": docutils.__version__,
        "sphinx_version": sphinx.__version__,
        "python_version": sys.version.split()[0],
        "unicodedata_version": unicodedata.unidata_version,
        "actions": ACTIONS,
        "languages": LANGUAGES,
        "educate_tokens": educate_rows,
        "educate_quotes": quote_rows,
        "random": {
            "seed": 0,
            "alphabet": ALPHABET,
            "settings": random_settings,
            "rows": random_rows,
        },
        "random_context": {
            "seed": 1,
            "settings": context_settings,
            "rows": context_rows,
        },
        "normalize_language_tag": tag_rows,
        "quote_sets": quote_sets,
        "escapes": escapes,
        "insert_escape_backslashes": inserted,
        "char_classes": char_classes(),
    }


def dump(value, indent=0):
    """JSON with one line per row: the top levels indented, each element of
    a list of objects on a line of its own, so a regeneration diffs row by
    row. ASCII only, so that no editor turns a no-break space into a
    space."""
    pad = "  " * indent
    if isinstance(value, dict) and indent < 2:
        items = [f'{pad}  {json.dumps(k)}: {dump(v, indent + 1)}' for k, v in sorted(value.items())]
        return "{\n" + ",\n".join(items) + f"\n{pad}}}"
    if isinstance(value, list) and value and all(isinstance(v, dict) for v in value):
        items = [f"{pad}  {json.dumps(v, sort_keys=True)}" for v in value]
        return "[\n" + ",\n".join(items) + f"\n{pad}]"
    return json.dumps(value, sort_keys=True)


def rust_str(s):
    return '"' + "".join(c if 0x20 <= ord(c) < 0x7F and c not in '"\\' else f"\\u{{{ord(c):x}}}" for c in s) + '"'


def tables():
    quotes = sq.smartchars.quotes
    assert len(quotes) == 75, len(quotes)
    word = runs(code_points(lambda c: re.fullmatch(r"\w", c) is not None))
    out = []
    out.append(
        "//! GENERATED by tools/gen_smartquotes_fixture.py --tables from docutils\n"
        f"//! {docutils.__version__} (`docutils.utils.smartquotes.smartchars.quotes`) and\n"
        f"//! Python {sys.version.split()[0]}'s `re` (Unicode {unicodedata.unidata_version}). Do not edit by hand.\n"
        "//! Regenerate: `PYTHONNOUSERSITE=1 uv run --locked --python 3.12 \\\n"
        "//!     --project tools/oracle_profiles/core \\\n"
        "//!     python tools/gen_smartquotes_fixture.py --tables`, then `cargo fmt --all`.\n\n"
    )
    out.append(
        "/// One language's quotes, `smartchars.quotes[language]` unpacked as\n"
        "/// `opquote, cpquote, osquote, csquote`: the opening and closing primary\n"
        "/// quote (`\"`) and secondary quote (`'`). Each is a string, not a\n"
        "/// character: French puts a no-break space inside its guillemets.\n"
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n"
        "pub struct QuoteSet {\n"
        "    pub op: &'static str,\n"
        "    pub cp: &'static str,\n"
        "    pub os: &'static str,\n"
        "    pub cs: &'static str,\n"
        "}\n\n"
    )
    out.append(
        f"/// `smartchars.quotes` (`SP/docutils/utils/smartquotes.py:411-497`): the {len(quotes)}\n"
        "/// languages docutils has quotes for, sorted by tag.\n"
        "pub static QUOTES: &[(&str, QuoteSet)] = &[\n"
    )
    for tag in sorted(quotes):
        op, cp, os_, cs = quotes[tag]
        out.append(
            f"    ({rust_str(tag)}, QuoteSet {{ op: {rust_str(op)}, cp: {rust_str(cp)}, "
            f"os: {rust_str(os_)}, cs: {rust_str(cs)} }}),\n"
        )
    out.append("];\n\n")
    out.append(
        "/// Python's `\\w` on a `str` pattern, `str.isalnum()` or `_`, as sorted\n"
        "/// inclusive `(first, last)` code point runs.\n"
        "pub static PY_WORD_RUNS: &[(u32, u32)] = &[\n"
    )
    for first, last in word:
        out.append(f"    (0x{first:X}, 0x{last:X}),\n")
    out.append("];\n")
    TABLES_PATH.write_text("".join(out), encoding="utf-8")
    print(f"wrote {TABLES_PATH}: {len(quotes)} languages, {len(word)} word runs")


def load_sphinx_generator():
    """tools/gen_sphinx_fixture.py, imported by path: its `CASES` and the
    harness that reads them (`make_app`, `probe`, `normalize`)."""
    spec = importlib.util.spec_from_file_location("gen_sphinx_fixture", SPHINX_GENERATOR)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def read_corpus(gs, smartquotes):
    """Every case of `gs.CASES` read through its harness with `smartquotes`
    as given and `keep_warnings = True`, one app per distinct `conf`, as the
    generator itself groups them: `{name: (pseudo_xml, warning lines,
    effective settings)}`, the warning lines those the case's read wrote,
    with the source path spelled as in the pseudo-XML."""
    gs.CONFOVERRIDES = {"smartquotes": smartquotes, "keep_warnings": True}
    groups = {}
    for case in gs.CASES:
        family, name, rst, conf = gs.case_parts(case)
        key = json.dumps(conf, sort_keys=True)
        groups.setdefault(key, (conf, []))[1].append((f"{family}.{name}", rst))
    results = {}
    for key in sorted(groups):
        conf, cases = groups[key]
        base = Path(tempfile.mkdtemp(prefix="smartquotes_variant_")).resolve() / "src"
        with docutils_namespace(), patch_docutils(str(base)):
            app = gs.make_app(base, conf)
            try:
                settings = {
                    "smartquotes": app.config.smartquotes,
                    "smartquotes_action": app.config.smartquotes_action,
                    "language": app.config.language,
                    "keep_warnings": app.config.keep_warnings,
                    "builder": app.builder.name,
                }
                assert settings["smartquotes"] is smartquotes, settings
                for name, rst in cases:
                    start = len(app._warning.getvalue())
                    doctree = gs.probe(app, base, rst)
                    warnings = app._warning.getvalue()[start:]
                    warnings = warnings.replace(str(base / "index.rst"), gs.SOURCE_TOKEN)
                    # Another file of the srcdir a warning names (the
                    # download collector's `download file not readable`,
                    # wave 5a T17a) is spelled under `<SRCDIR>`; such a
                    # warning is never SmartQuotes', so the variant records
                    # it in no case.
                    warnings = warnings.replace(f"{base}{os.sep}", "<SRCDIR>/")
                    assert str(base) not in warnings, f"{name}: srcdir leaked:\n{warnings}"
                    results[name] = (
                        gs.normalize(doctree.pformat(), base),
                        warnings.splitlines(),
                        settings,
                    )
            finally:
                app.cleanup()
                shutil.rmtree(base.parent, ignore_errors=True)
    return results


def variant():
    """The smart-quotes variant of the snippet corpus (report §6.3)."""
    gs = load_sphinx_generator()
    base_fixture = json.loads((REPO_ROOT / BASE_FIXTURE).read_text(encoding="utf-8"))
    committed = {case["name"]: case["pseudo_xml"] for case in base_fixture["cases"]}
    corpus = [gs.case_parts(case) for case in gs.CASES]
    names = [f"{family}.{name}" for family, name, _, _ in corpus]
    assert names == list(committed), (
        f"{BASE_FIXTURE} is not tools/gen_sphinx_fixture.py's corpus: regenerate it first"
    )

    off = read_corpus(gs, False)
    on = read_corpus(gs, True)
    again = read_corpus(gs, True)
    assert on == again, "two smart-quotes reads of the corpus differ"

    cases = []
    settings = None
    for (family, name, rst, conf), case_name in zip(corpus, names):
        off_xml, off_warnings, _ = off[case_name]
        on_xml, on_warnings, on_settings = on[case_name]
        assert off_xml == committed[case_name], (
            f"{case_name}: the smartquotes-off read is not {BASE_FIXTURE}'s tree"
        )
        if not conf:
            settings = on_settings
        # The warning lines the smart-quotes read printed and the other did
        # not, in the order it printed them; none may go missing.
        budget = Counter(off_warnings)
        added = []
        for line in on_warnings:
            if budget[line]:
                budget[line] -= 1
            else:
                added.append(line)
        assert not +budget, f"{case_name}: smart quotes removed warnings: {sorted(+budget)}"
        assert all(SMARTQUOTES_WARNING in line for line in added), (
            f"{case_name}: a warning that is not SmartQuotes': {added}"
        )
        if on_xml == off_xml and not added:
            continue
        cases.append({
            "name": case_name,
            "rst": rst,
            "conf": conf,
            "pseudo_xml_off": off_xml,
            "pseudo_xml": on_xml,
            "smartquotes_warnings": "".join(f"{line}\n" for line in added),
        })
    assert settings == {
        "smartquotes": True,
        "smartquotes_action": "qDe",
        "language": "en",
        "keep_warnings": True,
        "builder": "dummy",
    }, settings
    assert len(cases) >= VARIANT_FLOOR, f"variant degenerated: {len(cases)} cases"
    return {
        "docutils_version": docutils.__version__,
        "sphinx_version": sphinx.__version__,
        "generator": "tools/gen_smartquotes_fixture.py --variant",
        "base_fixture": BASE_FIXTURE,
        "settings": settings,
        "cases": cases,
    }


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--units", action="store_true", help="write tests/fixtures/smartquotes_units.json")
    mode.add_argument("--tables", action="store_true", help="write src/transforms/smartquotes/tables.rs")
    mode.add_argument(
        "--variant",
        action="store_true",
        help="write tests/fixtures/sphinx_smartquotes_differential.json",
    )
    args = parser.parse_args(argv)
    if args.tables:
        tables()
        return 0
    if args.variant:
        fixture = variant()
        with open(VARIANT_PATH, "w", encoding="utf-8") as f:
            json.dump(fixture, f, indent=2, sort_keys=True, ensure_ascii=False)
            f.write("\n")
        print(f"wrote {VARIANT_PATH}: {len(fixture['cases'])} cases")
        return 0
    fixture = units()
    UNITS_PATH.write_text(dump(fixture) + "\n", encoding="utf-8")
    print(
        f"wrote {UNITS_PATH}: {len(fixture['educate_tokens'])} educate_tokens rows, "
        f"{len(fixture['educate_quotes'])} educate_quotes rows, "
        f"{len(fixture['random']['rows'])} random strings x {len(fixture['random']['settings'])} settings, "
        f"{len(fixture['random_context']['rows'])} random sequences x "
        f"{len(fixture['random_context']['settings'])} settings"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
