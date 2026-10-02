#!/usr/bin/env python3
"""Generate the Sphinx 9.1 English search-language fixture.

Regenerate with:

    PYTHONNOUSERSITE=1 uv run --no-project --offline --python 3.12 \
        --with 'sphinx==9.1.0' --with 'docutils==0.22.4' \
        python tools/gen_search_english_fixture.py

Generate the complete local oracle instead of the compact committed fixture:

    PYTHONNOUSERSITE=1 uv run --no-project --offline --python 3.12 \
        --with 'sphinx==9.1.0' --with 'docutils==0.22.4' \
        python tools/gen_search_english_fixture.py --full \
        --output tests/fixtures/.search_english_full.tsv

The fixture is generated from Sphinx's SearchEnglish class rather than from a
hand-written Porter example. The committed fixture contains every token in the
RST fixtures, focused review probes and Snowball edge cases, plus a deterministic
frequency-ranked sample of the larger pinned-source corpus. ``--full`` emits
all unique words from that corpus for local verification.
"""

import argparse
from collections import Counter
import sysconfig
from pathlib import Path

import docutils
import sphinx
from sphinx.search.en import SearchEnglish

EXPECTED_DOCUTILS = "0.22.4"
EXPECTED_SPHINX = "9.1.0"

assert docutils.__version__ == EXPECTED_DOCUTILS, (
    f"docutils {docutils.__version__} != {EXPECTED_DOCUTILS}; "
    "regenerate with the pinned command in the module docstring"
)
assert sphinx.__version__ == EXPECTED_SPHINX, (
    f"sphinx {sphinx.__version__} != {EXPECTED_SPHINX}; "
    "regenerate with the pinned command in the module docstring"
)


STANDARD_ENGLISH_SAMPLE = r"""
When in the Course of human events, it becomes necessary for one people to
dissolve the political bands which have connected them with another, and to
assume among the powers of the earth, the separate and equal station to which
the Laws of Nature and of Nature's God entitle them, a decent respect to the
opinions of mankind requires that they should declare the causes which impel
them to the separation.

We hold these truths to be self-evident, that all men are created equal, that
they are endowed by their Creator with certain unalienable Rights, that among
these are Life, Liberty and the pursuit of Happiness. That to secure these
rights, Governments are instituted among Men, deriving their just powers from
the consent of the governed, That whenever any Form of Government becomes
destructive of these ends, it is the Right of the People to alter or to abolish
it, and to institute new Government, laying its foundation on such principles
and organizing its powers in such form, as to them shall seem most likely to
effect their Safety and Happiness. Prudence, indeed, will dictate that
Governments long established should not be changed for light and transient
causes; and accordingly all experience hath shewn, that mankind are more
disposed to suffer, while evils are sufferable, than to right themselves by
abolishing the forms to which they are accustomed. But when a long train of
abuses and usurpations, pursuing invariably the same Object evinces a design
to reduce them under absolute Despotism, it is their right, it is their duty,
to throw off such Government, and to provide new Guards for their future
security.

The quick brown fox jumps over the lazy dog. A bright student writes careful
sentences, studies changing languages, and compares historical references.
Reliable systems require testing, reasoning, maintenance, and responsible
engineering. They preserve searchable documents, useful examples, practical
guidance, and understandable explanations for readers everywhere.

relational conditional rational valenci hesitanci digitizer conformabli
radicalli differentli vileli analogousli vietnamization predication operator
feudalism decisiveness hopefulness callousness formaliti sensitiviti sensibiliti
triplicate formative formalize electriciti electrical hopeful goodness revival
allowance inference airliner gyroscopic adjustable defensible irritant
replacement adjustment dependent adoption homologou communism activate angulariti
homologous effective bowdlerize probate rate cease controlling generalization
reference permit generically generous generously inventories whatever only
running walked cats skies tying lying news atlas early gently singly ugly
"""

REQUIRED_WORDS = [
    "skis",
    "idly",
    "sky",
    "howe",
    "cosmos",
    "bias",
    "andes",
    "evening",
    "dying",
    "vying",
    "inning",
    "outing",
    "canning",
    "herring",
    "earring",
    "geologist",
    "analogist",
    "carelessli",
    "senselessli",
    "inventory",
    "only",
    "whatever",
    "'s",
    "dogs'",
    "children's",
    "'quoted",
    "O'Reilly",
    "½",
    "Ⅳ",
    "¹",
    # Snowball's English exception lists.
    "skis",
    "skies",
    "dying",
    "lying",
    "tying",
    "idly",
    "gently",
    "ugly",
    "early",
    "only",
    "singly",
    "sky",
    "news",
    "howe",
    "atlas",
    "cosmos",
    "bias",
    "andes",
    "inning",
    "outing",
    "canning",
    "herring",
    "earring",
    "proceed",
    "exceed",
    "succeed",
    # Step 1b probes.
    "evening",
    "pasting",
    "filing",
    "failing",
    "hopping",
    "tanned",
    "falling",
    "hissing",
    "fizz",
    # Step 2 suffix probes, including the reviewer probes.
    "relational",
    "conditional",
    "rational",
    "valenci",
    "hesitanci",
    "digitizer",
    "conformabli",
    "radicalli",
    "differentli",
    "vileli",
    "analogousli",
    "vietnamization",
    "predication",
    "operator",
    "feudalism",
    "decisiveness",
    "hopefulness",
    "callousness",
    "formaliti",
    "sensitiviti",
    "sensibiliti",
    "triplicate",
    "formative",
    "formalize",
    "electriciti",
    "electrical",
    "hopeful",
    "goodness",
    "revival",
    "allowance",
    "inference",
    "airliner",
    "gyroscopic",
    "adjustable",
    "defensible",
    "irritant",
    "replacement",
    "adjustment",
    "dependent",
    "adoption",
    "homologou",
    "communism",
    "activate",
    "angulariti",
    "homologous",
    "effective",
    "bowdlerize",
    "probate",
    "rate",
    "cease",
    "controlling",
    "generalization",
    "reference",
    "permit",
    "generically",
    "generous",
    "generously",
    "geologist",
    "analogist",
    "carelessli",
    "senselessli",
]

SOURCE_SUFFIXES = {
    ".cfg",
    ".css",
    ".html",
    ".ini",
    ".js",
    ".json",
    ".md",
    ".py",
    ".pyi",
    ".rst",
    ".svg",
    ".toml",
    ".txt",
    ".xml",
    ".yaml",
    ".yml",
}


def add_source_words(english: SearchEnglish, root: Path, words: list[str]) -> dict:
    file_count = 0
    token_count = 0
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.suffix.lower() not in SOURCE_SUFFIXES:
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        file_count += 1
        tokens = english.split(text)
        token_count += len(tokens)
        words.extend(tokens)
    return {"files": file_count, "tokens": token_count}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--full",
        action="store_true",
        help="write every unique word instead of the capped PR fixture",
    )
    parser.add_argument(
        "--limit",
        type=int,
        default=25_000,
        help="number of words in compact mode (default: 25000)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="output path, relative to the repository root unless absolute",
    )
    args = parser.parse_args()
    if args.limit <= 0:
        parser.error("--limit must be positive")
    return args


def fixture_row(english: SearchEnglish, word: str) -> str:
    stemmed = english.stem(word)
    term = stemmed if english.word_filter(stemmed) else None
    return f"{word}\t{stemmed}\t{term or '-'}"


def main() -> int:
    args = parse_args()
    root = Path(__file__).resolve().parent.parent
    english = SearchEnglish({})
    rst_paths = sorted((root / "tests" / "fixtures").rglob("*.rst"))
    supplemental_paths = [
        root / "README.md",
        root / "CHANGELOG.md",
        root / "CONTRIBUTING.md",
        root / "ROADMAP.md",
        root / "docs" / "README.md",
        root / "docs" / "QUICK_START.md",
    ]
    words: list[str] = []
    rst_words: list[str] = []
    for path in rst_paths:
        split_words = english.split(path.read_text(encoding="utf-8"))
        words.extend(split_words)
        rst_words.extend(split_words)
    words.extend(english.split(STANDARD_ENGLISH_SAMPLE))
    for path in supplemental_paths:
        words.extend(english.split(path.read_text(encoding="utf-8")))
    source_roots = {
        "python": Path(sysconfig.get_paths()["stdlib"]),
        "sphinx": Path(sphinx.__file__).resolve().parent,
        "docutils": Path(docutils.__file__).resolve().parent,
    }
    source_stats = {
        label: add_source_words(english, path, words)
        for label, path in source_roots.items()
    }
    words.extend(REQUIRED_WORDS)

    word_counts = Counter(words)
    all_words = set(word_counts)
    required_words = set(rst_words) | set(REQUIRED_WORDS)
    if not required_words <= all_words:
        raise AssertionError("required words were not added to the extraction")
    if not args.full and len(required_words) > args.limit:
        raise ValueError(
            f"{len(required_words)} required words exceed compact limit {args.limit}"
        )

    if args.full:
        selected_words = sorted(all_words)
    else:
        sample_size = args.limit - len(required_words)
        candidates = sorted(
            all_words - required_words,
            key=lambda word: (-word_counts[word], word),
        )
        selected_words = sorted(required_words | set(candidates[:sample_size]))

    output_name = (
        ".search_english_full.tsv" if args.full else "search_english.tsv"
    )
    out_path = args.output or (root / "tests" / "fixtures" / output_name)
    if not out_path.is_absolute():
        out_path = root / out_path
    out_path.parent.mkdir(parents=True, exist_ok=True)
    lines = [
        "# sphinx=9.1.0",
        "# docutils=0.22.4",
        "# generator=tools/gen_search_english_fixture.py",
        f"# corpus={'full' if args.full else 'compact'}",
        f"# words={len(selected_words)}",
        "# columns=word<TAB>stem<TAB>term",
        "# dropped-term=-",
    ]
    lines.extend(fixture_row(english, word) for word in selected_words)
    out_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(
        f"wrote {out_path}: {len(selected_words)} words "
        f"({len(all_words)} full unique; {len(required_words)} required; "
        f"source stats: {source_stats})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
