#!/usr/bin/env python3
"""Generate the pattern-matching differential fixture.

Runs a deterministic corpus of glob patterns and paths through Sphinx's own
``sphinx.util.matching.patmatch`` and records every verdict, so the Rust
translator in src/matching.rs can be diffed against real Sphinx behavior
(tests/pattern_differential.rs).

Regenerate with:

    uv run --python 3.12 --with 'sphinx>=9.1,<9.2' python tools/gen_pattern_fixture.py
"""

import json
from pathlib import Path

import sphinx
from sphinx.util.matching import patmatch

PATTERNS = [
    "*",
    "**",
    "?",
    "*.rst",
    "**.rst",
    "**/*.rst",
    "**/index.rst",
    "docs/**",
    "docs/**/*.rst",
    "foo/**/bar",
    "**/bar",
    "a**b",
    "**/**",
    "***",
    "_build/**",
    ".*/**",
    "chapter?.rst",
    "[abc].rst",
    "[!abc].rst",
    "[^abc].rst",
    "[a-z]x",
    "[!a-z]x",
    r"[\d]x",
    "[abc",  # unclosed class: the '[' is a literal
    "[]a]",
    "[!]a]",
    "a+b.rst",
    "a(b).rst",
    "a{b}.rst",
    "a|b",
    "Thumbs.db",
]

PATHS = [
    "index.rst",
    "a.rst",
    "foo",
    "foo/bar",
    "foo/bar.rst",
    "foo/x/bar",
    "foo/x/y/bar",
    "docs/api.rst",
    "docs/a/b.rst",
    "_build/x",
    "_build/a/b",
    ".git/config",
    "ab",
    "axb",
    "axx/yyb",
    "chapter1.rst",
    "chapterX.rst",
    "bx",
    "dx",
    "5x",
    "^x",
    "a-z",
    "src/code.py",
    "a/b/c/d/e.rst",
    "Thumbs.db",
    "x/Thumbs.db",
    "[a].rst",
    "]a]",
]

# Pairs outside the cross-product that pin down specific behaviors
# (literal regex metacharacters, in-class backslashes, ']' edge cases).
TARGETED = [
    ("a+b.rst", "a+b.rst"),
    ("a(b).rst", "a(b).rst"),
    ("a{b}.rst", "a{b}.rst"),
    ("a|b", "a|b"),
    ("[abc", "[abc"),
    (r"[\d]x", "\\x"),
    ("[]a]", "]"),
    ("[]a]", "a"),
    ("[!]a]", "xa]"),
    ("[!abc].rst", "d.rst"),
    ("[^abc].rst", "^.rst"),
    ("chapter?.rst", "chapter10.rst"),
    ("**/index.rst", "docs/index.rst"),
]


def main() -> None:
    pairs = {(pattern, path) for pattern in PATTERNS for path in PATHS}
    pairs.update(TARGETED)

    cases = [
        {"pattern": pattern, "path": path, "match": bool(patmatch(path, pattern))}
        for pattern, path in sorted(pairs)
    ]

    true_count = sum(case["match"] for case in cases)
    false_count = len(cases) - true_count
    # The corpus must not silently degenerate into one-sided coverage.
    assert true_count >= 100, f"only {true_count} matching cases"
    assert false_count >= 100, f"only {false_count} non-matching cases"

    fixture = {
        "sphinx_version": sphinx.__version__,
        "generator": "tools/gen_pattern_fixture.py",
        "cases": cases,
    }
    out_path = (
        Path(__file__).resolve().parent.parent
        / "tests"
        / "fixtures"
        / "pattern_differential.json"
    )
    out_path.write_text(json.dumps(fixture, indent=2, sort_keys=True) + "\n")
    print(
        f"wrote {len(cases)} cases "
        f"({true_count} match / {false_count} non-match) to {out_path}"
    )


if __name__ == "__main__":
    main()
